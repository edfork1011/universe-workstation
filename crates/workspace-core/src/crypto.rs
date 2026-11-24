//! Ed25519 identity keys, self-signed X.509v3 certificates, 6-digit SAS derivation,
//! and strict mTLS public-key pinning verifiers (`rustls`).

use std::fmt;
use std::sync::Arc;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use ed25519_dalek::pkcs8::DecodePrivateKey;
use ed25519_dalek::pkcs8::EncodePrivateKey;
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use hkdf::Hkdf;
use rand::RngCore;
use rand::rngs::OsRng;
use rcgen::string::Ia5String;
use rcgen::{CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, PKCS_ED25519, PublicKeyData, SanType, SerialNumber, SigningKey as RcgenSigningKey};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{
    CryptoProvider, WebPkiSupportedAlgorithms, verify_tls12_signature, verify_tls13_signature,
};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName, UnixTime};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::{ClientConfig, DigitallySignedStruct, ServerConfig, SignatureScheme};
use sha2::{Digest, Sha256};
use thiserror::Error;
use x509_parser::prelude::{FromDer, X509Certificate};
use zeroize::Zeroizing;

use crate::protocol::{DiscoveryBeacon, Platform};

/// Ed25519 SubjectPublicKeyInfo algorithm OID (`1.3.101.112`).
const OID_ED25519: &str = "1.3.101.112";

#[derive(Debug, Error)]
pub enum CryptoError {
    #[error("key generation or PKCS#8 error: {0}")]
    Key(String),
    #[error("X.509 certificate error: {0}")]
    Certificate(String),
    #[error("TLS configuration error: {0}")]
    Tls(String),
    #[error("invalid signature or encoding: {0}")]
    Signature(String),
}

/// Install rustls's `ring` crypto provider once per process.
pub fn install_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

fn crypto_provider() -> Result<Arc<CryptoProvider>, CryptoError> {
    install_crypto_provider();
    CryptoProvider::get_default()
        .cloned()
        .ok_or_else(|| CryptoError::Tls("failed to obtain rustls default CryptoProvider".into()))
}

/// Derive canonical `device_id` (32-char lowercase hex) from a 32-byte Ed25519 public key.
pub fn derive_device_id(pubkey: &[u8; 32]) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"me.edfork.workspace.id.v1");
    hasher.update(pubkey);
    let digest = hasher.finalize();
    let bytes = &digest.as_bytes()[..16];
    hex_encode(bytes)
}

/// Format a human-readable 16-char uppercase verification fingerprint (`XXXX-XXXX-XXXX-XXXX`).
pub fn format_pubkey_fingerprint(pubkey: &[u8; 32]) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"me.edfork.workspace.fp.v1");
    hasher.update(pubkey);
    let digest = hasher.finalize();
    let bytes = &digest.as_bytes()[..8];
    format!(
        "{:02X}{:02X}-{:02X}{:02X}-{:02X}{:02X}-{:02X}{:02X}",
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7]
    )
}

pub fn hex_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        use std::fmt::Write;
        let _ = write!(out, "{b:02x}");
    }
    out
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    hex_encode(&digest)
}

pub fn decode_pubkey_b64(b64: &str) -> Result<[u8; 32], CryptoError> {
    let bytes = STANDARD
        .decode(b64.trim())
        .map_err(|e| CryptoError::Key(format!("invalid base64 pubkey: {e}")))?;
    bytes
        .try_into()
        .map_err(|_| CryptoError::Key("Ed25519 public key must be exactly 32 bytes".into()))
}

pub fn encode_pubkey_b64(pubkey: &[u8; 32]) -> String {
    STANDARD.encode(pubkey)
}

/// Compute symmetric 6-digit SAS code (`"XXX YYY"`) from both parties' public keys and nonces.
pub fn compute_sas_code(
    pk_a: &[u8; 32],
    pk_b: &[u8; 32],
    initiator_nonce: &[u8; 32],
    responder_nonce: &[u8; 32],
) -> String {
    let (first_pk, second_pk) = if pk_a <= pk_b {
        (pk_a, pk_b)
    } else {
        (pk_b, pk_a)
    };
    let mut ikm = [0u8; 64];
    ikm[..32].copy_from_slice(first_pk);
    ikm[32..].copy_from_slice(second_pk);

    let mut salt = [0u8; 64];
    salt[..32].copy_from_slice(initiator_nonce);
    salt[32..].copy_from_slice(responder_nonce);

    let hk = Hkdf::<Sha256>::new(Some(&salt), &ikm);
    let mut okm = [0u8; 4];
    hk.expand(b"me.edfork.workspace.sas.v1", &mut okm)
        .expect("4-byte HKDF expansion never fails");
    let code = u32::from_be_bytes(okm) % 1_000_000;
    format!("{:03} {:03}", code / 1000, code % 1000)
}

pub fn pair_request_transcript(
    session_id: &str,
    initiator_id: &str,
    initiator_pk: &[u8; 32],
    initiator_nonce: &[u8; 32],
) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"me.edfork.workspace.pair-req.v1");
    hasher.update(&(session_id.len() as u32).to_be_bytes());
    hasher.update(session_id.as_bytes());
    hasher.update(&(initiator_id.len() as u32).to_be_bytes());
    hasher.update(initiator_id.as_bytes());
    hasher.update(initiator_pk);
    hasher.update(initiator_nonce);
    *hasher.finalize().as_bytes()
}

pub fn pair_response_transcript(
    session_id: &str,
    initiator_id: &str,
    responder_id: &str,
    initiator_pk: &[u8; 32],
    responder_pk: &[u8; 32],
    initiator_nonce: &[u8; 32],
    responder_nonce: &[u8; 32],
) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"me.edfork.workspace.pair-resp.v1");
    hasher.update(&(session_id.len() as u32).to_be_bytes());
    hasher.update(session_id.as_bytes());
    hasher.update(&(initiator_id.len() as u32).to_be_bytes());
    hasher.update(initiator_id.as_bytes());
    hasher.update(&(responder_id.len() as u32).to_be_bytes());
    hasher.update(responder_id.as_bytes());
    hasher.update(initiator_pk);
    hasher.update(responder_pk);
    hasher.update(initiator_nonce);
    hasher.update(responder_nonce);
    *hasher.finalize().as_bytes()
}

/// Predicate invoked during TLS 1.3 client-cert verification to decide whether a public key is trusted.
pub type TrustPredicate = Arc<dyn Fn(&[u8; 32]) -> bool + Send + Sync>;

#[allow(clippy::too_many_arguments)]
pub fn pair_confirm_transcript(
    session_id: &str,
    initiator_id: &str,
    responder_id: &str,
    initiator_pk: &[u8; 32],
    responder_pk: &[u8; 32],
    initiator_nonce: &[u8; 32],
    responder_nonce: &[u8; 32],
    role_tag: &[u8],
) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"me.edfork.workspace.pair-confirm.v1");
    hasher.update(role_tag);
    hasher.update(&(session_id.len() as u32).to_be_bytes());
    hasher.update(session_id.as_bytes());
    hasher.update(&(initiator_id.len() as u32).to_be_bytes());
    hasher.update(initiator_id.as_bytes());
    hasher.update(&(responder_id.len() as u32).to_be_bytes());
    hasher.update(responder_id.as_bytes());
    hasher.update(initiator_pk);
    hasher.update(responder_pk);
    hasher.update(initiator_nonce);
    hasher.update(responder_nonce);
    *hasher.finalize().as_bytes()
}

pub fn beacon_signing_bytes(beacon: &DiscoveryBeacon) -> Result<Vec<u8>, CryptoError> {
    let pk = decode_pubkey_b64(&beacon.pubkey_b64)?;
    let mut out = Vec::with_capacity(128);
    out.extend_from_slice(b"me.edfork.workspace.beacon.v1");
    out.extend_from_slice(&beacon.protocol.to_be_bytes());
    out.extend_from_slice(&(beacon.device_id.len() as u32).to_be_bytes());
    out.extend_from_slice(beacon.device_id.as_bytes());
    out.extend_from_slice(&(beacon.device_name.len() as u32).to_be_bytes());
    out.extend_from_slice(beacon.device_name.as_bytes());
    out.extend_from_slice(beacon.platform.as_str().as_bytes());
    out.extend_from_slice(&beacon.tls_port.to_be_bytes());
    out.extend_from_slice(&pk);
    out.extend_from_slice(&beacon.timestamp_ms.to_be_bytes());
    Ok(out)
}

pub fn verify_ed25519_signature(
    pubkey: &[u8; 32],
    message: &[u8],
    signature_bytes: &[u8],
) -> Result<(), CryptoError> {
    let vk = VerifyingKey::from_bytes(pubkey)
        .map_err(|e| CryptoError::Signature(format!("invalid Ed25519 pubkey: {e}")))?;
    let sig_arr: [u8; 64] = signature_bytes
        .try_into()
        .map_err(|_| CryptoError::Signature("signature must be 64 bytes".into()))?;
    let sig = Signature::from_bytes(&sig_arr);
    vk.verify_strict(message, &sig)
        .map_err(|e| CryptoError::Signature(format!("Ed25519 signature verification failed: {e}")))
}

/// Extract raw 32-byte Ed25519 public key from a DER-encoded X.509 certificate.
pub fn extract_ed25519_pubkey_from_cert(cert_der: &[u8]) -> Result<[u8; 32], CryptoError> {
    let (rem, cert) = X509Certificate::from_der(cert_der)
        .map_err(|e| CryptoError::Certificate(format!("failed to parse X.509 cert: {e}")))?;
    if !rem.is_empty() {
        return Err(CryptoError::Certificate(
            "trailing bytes after X.509 certificate".into(),
        ));
    }
    let spki = cert.public_key();
    if spki.algorithm.algorithm.to_id_string() != OID_ED25519 {
        return Err(CryptoError::Certificate(format!(
            "unsupported certificate key algorithm OID `{}` (expected Ed25519)",
            spki.algorithm.algorithm
        )));
    }
    let raw: [u8; 32] = spki
        .subject_public_key
        .data
        .as_ref()
        .try_into()
        .map_err(|_| CryptoError::Certificate("Ed25519 SPKI key must be 32 bytes".into()))?;
    // Verify it lies on the curve / is a valid Ed25519 verifying key.
    VerifyingKey::from_bytes(&raw)
        .map_err(|e| CryptoError::Certificate(format!("invalid Ed25519 verifying key: {e}")))?;
    Ok(raw)
}

/// Adapter letting `ed25519_dalek::SigningKey` sign self-signed X.509 certificates for `rcgen`.
struct Ed25519RcgenSigner {
    signing_key: SigningKey,
    raw_pubkey: [u8; 32],
}

impl PublicKeyData for Ed25519RcgenSigner {
    fn der_bytes(&self) -> &[u8] {
        // `rcgen::serialize_public_key_der` wraps this slice in a BIT STRING inside SubjectPublicKeyInfo.
        &self.raw_pubkey
    }

    fn algorithm(&self) -> &'static rcgen::SignatureAlgorithm {
        &PKCS_ED25519
    }
}

impl RcgenSigningKey for Ed25519RcgenSigner {
    fn sign(&self, msg: &[u8]) -> Result<Vec<u8>, rcgen::Error> {
        let sig: Signature = self.signing_key.sign(msg);
        Ok(sig.to_bytes().to_vec())
    }
}

/// Persistent Ed25519 identity + bound X.509v3 certificate for this device.
pub struct DeviceIdentity {
    pub device_id: String,
    device_name: Arc<std::sync::RwLock<String>>,
    pub platform: Platform,
    signing_key: SigningKey,
    pubkey: [u8; 32],
    pkcs8_der: Zeroizing<Vec<u8>>,
    cert_der: CertificateDer<'static>,
}

impl Clone for DeviceIdentity {
    fn clone(&self) -> Self {
        Self {
            device_id: self.device_id.clone(),
            device_name: Arc::clone(&self.device_name),
            platform: self.platform,
            signing_key: self.signing_key.clone(),
            pubkey: self.pubkey,
            pkcs8_der: self.pkcs8_der.clone(),
            cert_der: self.cert_der.clone(),
        }
    }
}

impl fmt::Debug for DeviceIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DeviceIdentity")
            .field("device_id", &self.device_id)
            .field("device_name", &self.device_name())
            .field("platform", &self.platform)
            .field("fingerprint", &self.fingerprint())
            .finish()
    }
}

impl DeviceIdentity {
    pub fn device_name(&self) -> String {
        self.device_name
            .read()
            .expect("device_name lock poisoned")
            .clone()
    }

    pub fn set_device_name(&self, name: impl Into<String>) {
        *self.device_name.write().expect("device_name lock poisoned") = name.into();
    }
    pub fn generate(device_name: impl Into<String>, platform: Platform) -> Result<Self, CryptoError> {
        let signing_key = SigningKey::generate(&mut OsRng);
        Self::from_signing_key(signing_key, device_name.into(), platform, None)
    }

    pub fn from_stored(
        device_name: impl Into<String>,
        platform: Platform,
        pkcs8_der: &[u8],
        cert_der: &[u8],
    ) -> Result<Self, CryptoError> {
        let signing_key = SigningKey::from_pkcs8_der(pkcs8_der)
            .map_err(|e| CryptoError::Key(format!("invalid PKCS#8 Ed25519 key: {e}")))?;
        let expected_pk = signing_key.verifying_key().to_bytes();
        let cert_pk = extract_ed25519_pubkey_from_cert(cert_der)?;
        if cert_pk != expected_pk {
            return Err(CryptoError::Certificate(
                "stored X.509 certificate public key does not match stored private key".into(),
            ));
        }
        Self::from_signing_key(
            signing_key,
            device_name.into(),
            platform,
            Some(cert_der.to_vec()),
        )
    }

    fn from_signing_key(
        signing_key: SigningKey,
        device_name: String,
        platform: Platform,
        existing_cert_der: Option<Vec<u8>>,
    ) -> Result<Self, CryptoError> {
        let pubkey = signing_key.verifying_key().to_bytes();
        let device_id = derive_device_id(&pubkey);
        let pkcs8 = signing_key
            .to_pkcs8_der()
            .map_err(|e| CryptoError::Key(format!("failed to encode PKCS#8 key: {e}")))?;
        let pkcs8_der = Zeroizing::new(pkcs8.as_bytes().to_vec());

        let cert_der = match existing_cert_der {
            Some(bytes) => CertificateDer::from(bytes),
            None => build_self_signed_cert(&signing_key, pubkey, &device_id, &device_name)?,
        };

        Ok(Self {
            device_id,
            device_name: Arc::new(std::sync::RwLock::new(device_name)),
            platform,
            signing_key,
            pubkey,
            pkcs8_der,
            cert_der,
        })
    }

    pub fn pubkey_bytes(&self) -> [u8; 32] {
        self.pubkey
    }

    pub fn pubkey_b64(&self) -> String {
        encode_pubkey_b64(&self.pubkey)
    }

    pub fn fingerprint(&self) -> String {
        format_pubkey_fingerprint(&self.pubkey)
    }

    pub fn cert_der(&self) -> &CertificateDer<'static> {
        &self.cert_der
    }

    pub fn cert_sha256_hex(&self) -> String {
        sha256_hex(self.cert_der.as_ref())
    }

    pub fn pkcs8_der(&self) -> &[u8] {
        &self.pkcs8_der
    }

    pub fn private_key_der(&self) -> PrivateKeyDer<'static> {
        PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(self.pkcs8_der.to_vec()))
    }

    pub fn sign(&self, message: &[u8]) -> [u8; 64] {
        self.signing_key.sign(message).to_bytes()
    }

    pub fn sign_b64(&self, message: &[u8]) -> String {
        STANDARD.encode(self.sign(message))
    }

    pub fn sign_beacon(
        &self,
        tls_port: u16,
        timestamp_ms: u64,
    ) -> Result<DiscoveryBeacon, CryptoError> {
        let mut beacon = DiscoveryBeacon {
            protocol: crate::limits::PROTOCOL_VERSION,
            device_id: self.device_id.clone(),
            device_name: self.device_name(),
            platform: self.platform,
            tls_port,
            pubkey_b64: self.pubkey_b64(),
            timestamp_ms,
            signature_b64: String::new(),
        };
        let msg = beacon_signing_bytes(&beacon)?;
        beacon.signature_b64 = self.sign_b64(&msg);
        Ok(beacon)
    }
}

fn build_self_signed_cert(
    signing_key: &SigningKey,
    pubkey: [u8; 32],
    device_id: &str,
    device_name: &str,
) -> Result<CertificateDer<'static>, CryptoError> {
    let signer = Ed25519RcgenSigner {
        signing_key: signing_key.clone(),
        raw_pubkey: pubkey,
    };

    let mut params = CertificateParams::new(Vec::new())
        .map_err(|e| CryptoError::Certificate(format!("CertificateParams error: {e}")))?;

    let mut serial = [0u8; 16];
    OsRng.fill_bytes(&mut serial);
    serial[0] &= 0x7F;
    params.serial_number = Some(SerialNumber::from_slice(&serial));

    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, format!("Workspace {device_name}"));
    dn.push(DnType::OrganizationName, "me.edfork.workspace");
    params.distinguished_name = dn;
    params.not_before = rcgen::date_time_ymd(2020, 1, 1);
    params.not_after = rcgen::date_time_ymd(2099, 12, 31);
    params.extended_key_usages = vec![
        ExtendedKeyUsagePurpose::ServerAuth,
        ExtendedKeyUsagePurpose::ClientAuth,
    ];

    let san_uri = Ia5String::try_from(format!("urn:workspace:device:{device_id}"))
        .map_err(|e| CryptoError::Certificate(format!("SAN URI error: {e}")))?;
    params.subject_alt_names = vec![SanType::URI(san_uri)];

    let cert = params
        .self_signed(&signer)
        .map_err(|e| CryptoError::Certificate(format!("self_signed error: {e}")))?;
    Ok(cert.der().clone())
}

/// TLS server verification policy.
#[derive(Debug, Clone)]
enum ServerKeyPolicy {
    /// Operational or discovery-pinned connection: server Ed25519 pubkey MUST equal `expected`.
    ExactPubkey([u8; 32]),
}

#[derive(Debug)]
struct PinnedServerCertVerifier {
    policy: ServerKeyPolicy,
    algs: WebPkiSupportedAlgorithms,
}

impl ServerCertVerifier for PinnedServerCertVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let actual_pk = extract_ed25519_pubkey_from_cert(end_entity.as_ref())
            .map_err(|e| rustls::Error::General(format!("invalid server cert: {e}")))?;
        match self.policy {
            ServerKeyPolicy::ExactPubkey(expected) => {
                if actual_pk != expected {
                    return Err(rustls::Error::General(
                        "server public key does not match pinned peer identity".into(),
                    ));
                }
            }
        }
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(message, cert, dss, &self.algs)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(message, cert, dss, &self.algs)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algs.supported_schemes()
    }
}

/// Server-side client certificate verifier requiring valid Ed25519 client certs + TLS key proof.
/// When `trusted_only` is `Some(fn)`, untrusted clients are rejected directly during TLS handshake.
#[derive(Clone)]
struct WorkspaceClientCertVerifier {
    algs: WebPkiSupportedAlgorithms,
    trust_predicate: Option<TrustPredicate>,
}

impl fmt::Debug for WorkspaceClientCertVerifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WorkspaceClientCertVerifier")
            .field("trusted_only", &self.trust_predicate.is_some())
            .finish()
    }
}

impl ClientCertVerifier for WorkspaceClientCertVerifier {
    fn offer_client_auth(&self) -> bool {
        true
    }

    fn client_auth_mandatory(&self) -> bool {
        true
    }

    fn root_hint_subjects(&self) -> &[rustls::DistinguishedName] {
        &[]
    }

    fn verify_client_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _now: UnixTime,
    ) -> Result<ClientCertVerified, rustls::Error> {
        let pk = extract_ed25519_pubkey_from_cert(end_entity.as_ref())
            .map_err(|e| rustls::Error::General(format!("invalid client cert: {e}")))?;
        if let Some(predicate) = &self.trust_predicate
            && !predicate(&pk)
        {
            return Err(rustls::Error::General(
                "client certificate public key is not paired/trusted".into(),
            ));
        }
        Ok(ClientCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(message, cert, dss, &self.algs)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(message, cert, dss, &self.algs)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algs.supported_schemes()
    }
}

/// Build TLS 1.3-only mTLS server config accepting valid Ed25519 client certs (pairing + trusted routes).
pub fn build_mtls_server_config(identity: &DeviceIdentity) -> Result<Arc<ServerConfig>, CryptoError> {
    build_mtls_server_config_with_filter(identity, None)
}

/// Build TLS 1.3-only mTLS server config optionally rejecting untrusted peers at TLS handshake level.
pub fn build_mtls_server_config_with_filter(
    identity: &DeviceIdentity,
    trust_predicate: Option<TrustPredicate>,
) -> Result<Arc<ServerConfig>, CryptoError> {
    let provider = crypto_provider()?;
    let verifier = Arc::new(WorkspaceClientCertVerifier {
        algs: provider.signature_verification_algorithms,
        trust_predicate,
    });
    let mut config = ServerConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|e| CryptoError::Tls(e.to_string()))?
        .with_client_cert_verifier(verifier)
        .with_single_cert(vec![identity.cert_der().clone()], identity.private_key_der())
        .map_err(|e| CryptoError::Tls(e.to_string()))?;
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(Arc::new(config))
}

/// Build TLS 1.3-only mTLS client config pinning the target peer's exact Ed25519 public key.
pub fn build_mtls_client_config(
    identity: &DeviceIdentity,
    expected_server_pubkey: [u8; 32],
) -> Result<Arc<ClientConfig>, CryptoError> {
    let provider = crypto_provider()?;
    let verifier = Arc::new(PinnedServerCertVerifier {
        policy: ServerKeyPolicy::ExactPubkey(expected_server_pubkey),
        algs: provider.signature_verification_algorithms,
    });
    let mut config = ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|e| CryptoError::Tls(e.to_string()))?
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_client_auth_cert(vec![identity.cert_der().clone()], identity.private_key_der())
        .map_err(|e| CryptoError::Tls(e.to_string()))?;
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(Arc::new(config))
}

pub fn workspace_server_name(device_id: &str) -> ServerName<'static> {
    let sanitized: String = device_id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect();
    let host = if sanitized.is_empty() {
        "peer.workspace.local".to_string()
    } else {
        format!("{sanitized}.workspace.local")
    };
    ServerName::try_from(host).expect("valid synthetic SNI name")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_generation_and_cert_pubkey_roundtrip() {
        let id = DeviceIdentity::generate("TestLinux", Platform::Linux).unwrap();
        let extracted = extract_ed25519_pubkey_from_cert(id.cert_der().as_ref()).unwrap();
        assert_eq!(extracted, id.pubkey_bytes());
        assert_eq!(derive_device_id(&extracted), id.device_id);

        let restored = DeviceIdentity::from_stored(
            "TestLinux",
            Platform::Linux,
            id.pkcs8_der(),
            id.cert_der().as_ref(),
        )
        .unwrap();
        assert_eq!(restored.device_id, id.device_id);
        assert_eq!(restored.pubkey_bytes(), id.pubkey_bytes());
    }

    #[test]
    fn sas_code_is_symmetric_across_key_order() {
        let a = [1u8; 32];
        let b = [9u8; 32];
        let n1 = [3u8; 32];
        let n2 = [7u8; 32];

        let s1 = compute_sas_code(&a, &b, &n1, &n2);
        let s2 = compute_sas_code(&b, &a, &n1, &n2);
        assert_eq!(s1, s2);
        assert_eq!(s1.len(), 7); // "XXX YYY"

        // Swapping nonces intentionally changes SAS.
        let s3 = compute_sas_code(&a, &b, &n2, &n1);
        assert_ne!(s1, s3);
    }

    #[test]
    fn beacon_sign_and_verify() {
        let id = DeviceIdentity::generate("Phone", Platform::Android).unwrap();
        let beacon = id.sign_beacon(45877, 1_795_000_000_000).unwrap();
        let msg = beacon_signing_bytes(&beacon).unwrap();
        let sig = STANDARD.decode(&beacon.signature_b64).unwrap();
        let pk = decode_pubkey_b64(&beacon.pubkey_b64).unwrap();
        verify_ed25519_signature(&pk, &msg, &sig).unwrap();

        let mut tampered = beacon.clone();
        tampered.tls_port = 9999;
        let tampered_msg = beacon_signing_bytes(&tampered).unwrap();
        assert!(verify_ed25519_signature(&pk, &tampered_msg, &sig).is_err());
    }

    #[test]
    fn device_name_updates_live_across_cloned_identities() {
        let id = DeviceIdentity::generate("OPPO Find N6", Platform::Android).unwrap();
        let cloned = id.clone();
        let original_id = id.device_id.clone();

        id.set_device_name("Alice 的 OnePlus Open");
        assert_eq!(cloned.device_name(), "Alice 的 OnePlus Open");
        assert_eq!(cloned.device_id, original_id);

        let beacon = cloned.sign_beacon(45877, 1_795_000_000_000).unwrap();
        assert_eq!(beacon.device_name, "Alice 的 OnePlus Open");
        let msg = beacon_signing_bytes(&beacon).unwrap();
        let sig = STANDARD.decode(&beacon.signature_b64).unwrap();
        let pk = decode_pubkey_b64(&beacon.pubkey_b64).unwrap();
        verify_ed25519_signature(&pk, &msg, &sig).unwrap();
    }
}
