//! Signed UDP LAN discovery (`239.255.77.88`, subnet broadcasts, and trusted-peer unicast probes).

use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use socket2::{Domain, Protocol, Socket, Type};
use thiserror::Error;
use tokio::net::UdpSocket;
use tokio::sync::watch;
use tracing::{debug, trace, warn};

use crate::clipboard_model::now_timestamp_ms;
use crate::crypto::{
    DeviceIdentity, beacon_signing_bytes, decode_pubkey_b64, derive_device_id,
    format_pubkey_fingerprint, verify_ed25519_signature,
};
use crate::limits::{DEFAULT_DISCOVERY_PORT, DISCOVERY_MULTICAST_V4, PROTOCOL_VERSION};
use crate::protocol::{DiscoveredPeer, DiscoveryBeacon};
use crate::store::TrustRegistry;

const MAX_BEACON_BYTES: usize = 2048;
const STALE_PEER_MS: u64 = 15_000;
const ANNOUNCE_INTERVAL: Duration = Duration::from_secs(3);

#[derive(Debug, Error)]
pub enum DiscoveryError {
    #[error("discovery socket I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid multicast address: {0}")]
    InvalidMulticast(String),
}

#[derive(Debug, Clone)]
pub struct DiscoveryConfig {
    pub discovery_port: u16,
    pub tls_port: u16,
    pub announce_interval: Duration,
    pub stale_ttl_ms: u64,
    /// Optional explicit peer sockets (used in tests when binding ephemeral ports).
    pub extra_targets: Vec<SocketAddr>,
}

impl Default for DiscoveryConfig {
    fn default() -> Self {
        Self {
            discovery_port: DEFAULT_DISCOVERY_PORT,
            tls_port: crate::limits::DEFAULT_TLS_PORT,
            announce_interval: ANNOUNCE_INTERVAL,
            stale_ttl_ms: STALE_PEER_MS,
            extra_targets: Vec::new(),
        }
    }
}

/// Manages UDP LAN beacons, Ed25519 beacon verification, and discovered peer tracking.
#[derive(Clone)]
pub struct DiscoveryHandle {
    identity: Arc<DeviceIdentity>,
    trust: TrustRegistry,
    config: DiscoveryConfig,
    peers: Arc<RwLock<HashMap<String, DiscoveredPeer>>>,
    updates_tx: watch::Sender<u64>,
    announce_tx: tokio::sync::mpsc::Sender<()>,
    local_addr: SocketAddr,
}

impl DiscoveryHandle {
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.updates_tx.subscribe()
    }

    pub async fn announce_now(&self) {
        let _ = self.announce_tx.send(()).await;
    }

    /// Trigger an immediate announcement using an active beacon snapshot (or refresh trusted flags).
    pub fn list_peers(&self) -> Vec<DiscoveredPeer> {
        let now = now_timestamp_ms();
        let mut guard = self.peers.write().expect("Discovery peers lock poisoned");
        guard.retain(|_, peer| now.saturating_sub(peer.last_seen_ms) <= self.config.stale_ttl_ms);

        let mut list: Vec<DiscoveredPeer> = guard
            .values()
            .map(|p| {
                let mut cloned = p.clone();
                if let Ok(pk) = decode_pubkey_b64(&cloned.pubkey_b64) {
                    cloned.is_trusted = self.trust.is_pubkey_trusted(&pk);
                }
                cloned
            })
            .collect();
        list.sort_by(|a, b| a.device_name.cmp(&b.device_name).then(a.device_id.cmp(&b.device_id)));
        list
    }

    pub fn get_peer(&self, device_id: &str) -> Option<DiscoveredPeer> {
        let now = now_timestamp_ms();
        let guard = self.peers.read().expect("Discovery peers lock poisoned");
        let peer = guard.get(device_id)?;
        if now.saturating_sub(peer.last_seen_ms) > self.config.stale_ttl_ms {
            return None;
        }
        let mut cloned = peer.clone();
        if let Ok(pk) = decode_pubkey_b64(&cloned.pubkey_b64) {
            cloned.is_trusted = self.trust.is_pubkey_trusted(&pk);
        }
        Some(cloned)
    }

    /// Spawn the LAN discovery background loop returning `(DiscoveryHandle, JoinHandle)`.
    pub async fn start(
        identity: Arc<DeviceIdentity>,
        trust: TrustRegistry,
        config: DiscoveryConfig,
        mut shutdown_rx: watch::Receiver<bool>,
    ) -> Result<(Self, tokio::task::JoinHandle<()>), DiscoveryError> {
        let multicast_ip: Ipv4Addr = DISCOVERY_MULTICAST_V4
            .parse()
            .map_err(|_| DiscoveryError::InvalidMulticast(DISCOVERY_MULTICAST_V4.to_string()))?;

        let raw_sock = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
        raw_sock.set_reuse_address(true)?;
        raw_sock.set_broadcast(true)?;
        raw_sock.set_nonblocking(true)?;
        raw_sock.set_multicast_loop_v4(true)?;

        let bind_addr = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, config.discovery_port);
        raw_sock.bind(&bind_addr.into())?;

        let std_sock: std::net::UdpSocket = raw_sock.into();
        let udp = Arc::new(UdpSocket::from_std(std_sock)?);
        let local_addr = udp.local_addr()?;

        join_multicast_on_lan_interfaces(&udp, multicast_ip);

        let peers = Arc::new(RwLock::new(HashMap::new()));
        let (updates_tx, _) = watch::channel(0u64);
        let (announce_tx, mut announce_rx) = tokio::sync::mpsc::channel::<()>(8);

        let handle = Self {
            identity: identity.clone(),
            trust: trust.clone(),
            config: config.clone(),
            peers: peers.clone(),
            updates_tx: updates_tx.clone(),
            announce_tx,
            local_addr,
        };

        let task_handle = handle.clone();
        let join = tokio::spawn(async move {
            let mut buf = vec![0u8; MAX_BEACON_BYTES];
            let mut ticker = tokio::time::interval(config.announce_interval);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            let mut seq_gen = 0u64;

            loop {
                tokio::select! {
                    changed = shutdown_rx.changed() => {
                        if changed.is_err() || *shutdown_rx.borrow() {
                            break;
                        }
                    }
                    _ = ticker.tick() => {
                        // Retry multicast membership in case network interface came up after boot.
                        join_multicast_on_lan_interfaces(&udp, multicast_ip);
                        if task_handle.prune_stale() {
                            seq_gen = seq_gen.wrapping_add(1);
                            let _ = updates_tx.send(seq_gen);
                        }
                        send_beacon(&udp, &identity, &trust, &config, multicast_ip).await;
                    }
                    Some(()) = announce_rx.recv() => {
                        send_beacon(&udp, &identity, &trust, &config, multicast_ip).await;
                    }
                    recv_res = udp.recv_from(&mut buf) => {
                        match recv_res {
                            Ok((len, src)) => {
                                if handle_incoming_packet(&task_handle, &buf[..len], src) {
                                    seq_gen = seq_gen.wrapping_add(1);
                                    let _ = updates_tx.send(seq_gen);
                                }
                            }
                            Err(err) => {
                                warn!("UDP discovery recv error: {err}");
                            }
                        }
                    }
                }
            }
        });

        Ok((handle, join))
    }

    fn prune_stale(&self) -> bool {
        let now = now_timestamp_ms();
        let mut guard = self.peers.write().expect("Discovery peers lock poisoned");
        let before = guard.len();
        guard.retain(|_, peer| now.saturating_sub(peer.last_seen_ms) <= self.config.stale_ttl_ms);
        guard.len() != before
    }
}

fn join_multicast_on_lan_interfaces(udp: &UdpSocket, multicast_ip: Ipv4Addr) {
    let ifaces = collect_lan_interfaces_v4();
    if ifaces.is_empty() {
        if let Err(err) = udp.join_multicast_v4(multicast_ip, Ipv4Addr::UNSPECIFIED) {
            debug!("initial multicast join deferred: {err}");
        }
        return;
    }
    for iface in ifaces {
        if let Err(err) = udp.join_multicast_v4(multicast_ip, iface.ip) {
            trace!(
                "multicast join on {} ({}) skipped: {err}",
                iface.name, iface.ip
            );
        }
    }
}

async fn send_beacon_on_interface(
    iface: &LanInterfaceV4,
    payload: &[u8],
    target_port: u16,
    multicast_ip: Ipv4Addr,
    trusted_ips: &[Ipv4Addr],
    allow_all_trusted: bool,
) -> std::io::Result<()> {
    let raw = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    raw.set_reuse_address(true)?;
    raw.set_broadcast(true)?;
    raw.set_nonblocking(true)?;
    let _ = raw.set_multicast_loop_v4(false);
    let _ = raw.set_multicast_if_v4(&iface.ip);
    #[cfg(any(target_os = "android", target_os = "linux"))]
    {
        if let Err(err) = raw.bind_device(Some(iface.name.as_bytes())) {
            trace!("SO_BINDTODEVICE on {} skipped: {err}", iface.name);
        }
    }
    raw.bind(&SocketAddrV4::new(iface.ip, 0).into())?;

    let std_sock: std::net::UdpSocket = raw.into();
    let sock = UdpSocket::from_std(std_sock)?;

    let mut targets: HashSet<SocketAddrV4> = HashSet::new();
    targets.insert(SocketAddrV4::new(multicast_ip, target_port));
    targets.insert(SocketAddrV4::new(Ipv4Addr::BROADCAST, target_port));
    if let Some(bcast) = iface.broadcast {
        targets.insert(SocketAddrV4::new(bcast, target_port));
    }
    for &tip in trusted_ips {
        if allow_all_trusted || iface.contains_ipv4(tip) {
            targets.insert(SocketAddrV4::new(tip, target_port));
        }
    }

    for addr in targets {
        if let Err(err) = sock.send_to(payload, SocketAddr::V4(addr)).await {
            trace!(
                "discovery send via {} ({}) -> {addr} skipped: {err}",
                iface.name, iface.ip
            );
        }
    }
    Ok(())
}

async fn send_beacon(
    udp: &UdpSocket,
    identity: &DeviceIdentity,
    trust: &TrustRegistry,
    config: &DiscoveryConfig,
    multicast_ip: Ipv4Addr,
) {
    let Ok(beacon) = identity.sign_beacon(config.tls_port, now_timestamp_ms()) else {
        return;
    };
    let Ok(payload) = serde_json::to_vec(&beacon) else {
        return;
    };

    let target_port = config.discovery_port;
    if target_port != 0 {
        let trusted_v4: Vec<Ipv4Addr> = trust
            .list()
            .into_iter()
            .filter_map(|p| p.last_known_ip.and_then(|s| s.parse::<Ipv4Addr>().ok()))
            .collect();
        let ifaces = collect_lan_interfaces_v4();
        if !ifaces.is_empty() {
            let allow_all = ifaces.len() == 1;
            for iface in &ifaces {
                if let Err(err) = send_beacon_on_interface(
                    iface,
                    &payload,
                    target_port,
                    multicast_ip,
                    &trusted_v4,
                    allow_all,
                )
                .await
                {
                    trace!("interface beacon send on {} failed: {err}", iface.name);
                }
            }
        } else {
            let mut fallback_targets: HashSet<SocketAddr> = HashSet::new();
            fallback_targets.insert(SocketAddr::V4(SocketAddrV4::new(multicast_ip, target_port)));
            fallback_targets.insert(SocketAddr::V4(SocketAddrV4::new(
                Ipv4Addr::BROADCAST,
                target_port,
            )));
            for tip in trusted_v4 {
                fallback_targets.insert(SocketAddr::V4(SocketAddrV4::new(tip, target_port)));
            }
            for addr in fallback_targets {
                if let Err(err) = udp.send_to(&payload, addr).await {
                    trace!("discovery fallback send to {addr} skipped: {err}");
                }
            }
        }
    }

    for addr in &config.extra_targets {
        if let Err(err) = udp.send_to(&payload, *addr).await {
            trace!("discovery extra_target send to {addr} skipped: {err}");
        }
    }
}

fn handle_incoming_packet(handle: &DiscoveryHandle, bytes: &[u8], src: SocketAddr) -> bool {
    let Ok(beacon) = serde_json::from_slice::<DiscoveryBeacon>(bytes) else {
        return false;
    };
    if beacon.protocol != PROTOCOL_VERSION || beacon.tls_port == 0 {
        return false;
    }
    if beacon.device_id == handle.identity.device_id {
        return false;
    }
    if let IpAddr::V4(v4) = src.ip()
        && !v4.is_loopback()
        && collect_local_ipv4_addrs().contains(&v4)
    {
        // Ignore NAT/bridge-reflected beacons whose source IP matches a local host interface.
        return false;
    }

    let Ok(pubkey) = decode_pubkey_b64(&beacon.pubkey_b64) else {
        return false;
    };
    if derive_device_id(&pubkey) != beacon.device_id {
        return false;
    }
    let Ok(sig_bytes) = STANDARD.decode(beacon.signature_b64.trim()) else {
        return false;
    };
    let Ok(msg_bytes) = beacon_signing_bytes(&beacon) else {
        return false;
    };
    if verify_ed25519_signature(&pubkey, &msg_bytes, &sig_bytes).is_err() {
        warn!("rejected forged LAN beacon claiming device_id={}", beacon.device_id);
        return false;
    }

    let ip_str = src.ip().to_string();
    let is_trusted = handle.trust.is_pubkey_trusted(&pubkey);
    if is_trusted {
        let _ = handle.trust.update_seen_endpoint(
            &beacon.device_id,
            Some(&ip_str),
            beacon.tls_port,
            Some(&beacon.device_name),
        );
    }

    let now = now_timestamp_ms();
    let discovered = DiscoveredPeer {
        device_id: beacon.device_id.clone(),
        device_name: beacon.device_name,
        platform: beacon.platform,
        ip: ip_str,
        tls_port: beacon.tls_port,
        pubkey_b64: beacon.pubkey_b64,
        fingerprint: format_pubkey_fingerprint(&pubkey),
        is_trusted,
        last_seen_ms: now,
    };

    let mut guard = handle.peers.write().expect("Discovery peers lock poisoned");
    let before_len = guard.len();
    guard.retain(|id, p| {
        id == &beacon.device_id || !(p.ip == discovered.ip && p.tls_port == discovered.tls_port)
    });
    let evicted_same_endpoint = guard.len() != before_len;
    let changed = evicted_same_endpoint
        || match guard.get(&beacon.device_id) {
            Some(prev) => {
                prev.ip != discovered.ip
                    || prev.tls_port != discovered.tls_port
                    || prev.device_name != discovered.device_name
                    || prev.is_trusted != discovered.is_trusted
            }
            None => true,
        };
    guard.insert(beacon.device_id, discovered);
    changed
}

/// Enumerate active non-loopback IPv4 interface addresses assigned to this host.
pub fn collect_local_ipv4_addrs() -> HashSet<Ipv4Addr> {
    let mut addrs = HashSet::new();

    unsafe {
        let mut ifap: *mut libc::ifaddrs = std::ptr::null_mut();
        if libc::getifaddrs(&mut ifap) != 0 || ifap.is_null() {
            return addrs;
        }
        let mut cur = ifap;
        while !cur.is_null() {
            let ifa = &*cur;
            let flags = ifa.ifa_flags as i32;
            let is_up = (flags & libc::IFF_UP) != 0;
            let is_loopback = (flags & libc::IFF_LOOPBACK) != 0;

            if is_up
                && !is_loopback
                && !ifa.ifa_addr.is_null()
                && (*ifa.ifa_addr).sa_family as i32 == libc::AF_INET
            {
                let addr = &*(ifa.ifa_addr as *const libc::sockaddr_in);
                let ip = Ipv4Addr::from(u32::from_be(addr.sin_addr.s_addr));
                if !ip.is_unspecified() && !ip.is_loopback() {
                    addrs.insert(ip);
                }
            }
            cur = ifa.ifa_next;
        }
        libc::freeifaddrs(ifap);
    }

    addrs
}

/// Metadata describing a physical/local IPv4 LAN interface suitable for direct LAN discovery & mTLS.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LanInterfaceV4 {
    pub name: String,
    pub ip: Ipv4Addr,
    pub netmask: Ipv4Addr,
    pub broadcast: Option<Ipv4Addr>,
}

impl LanInterfaceV4 {
    /// Return true when `target` shares the same IPv4 subnet as this interface.
    pub fn contains_ipv4(&self, target: Ipv4Addr) -> bool {
        let mask = u32::from(self.netmask);
        mask != 0 && (u32::from(self.ip) & mask) == (u32::from(target) & mask)
    }
}

fn is_virtual_or_tunnel_ifname(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    if lower == "lo" {
        return true;
    }
    const VIRTUAL_PREFIXES: &[&str] = &[
        "tun",
        "tap",
        "utun",
        "tailscale",
        "wg",
        "ppp",
        "rmnet",
        "ccmni",
        "pdp_ip",
        "dummy",
        "docker",
        "br-",
        "veth",
        "virbr",
        "cni",
        "flannel",
        "calico",
        "kube",
        "zt",
        "clat",
        "ipsec",
        "vti",
        "sit",
        "ip6tnl",
        "gre",
    ];
    VIRTUAL_PREFIXES
        .iter()
        .any(|prefix| lower.starts_with(prefix))
}

/// Enumerate active physical/local LAN IPv4 interfaces, excluding VPN/tunnel/bridge/cellular devices.
pub fn collect_lan_interfaces_v4() -> Vec<LanInterfaceV4> {
    let mut out = Vec::new();
    let mut seen: HashSet<(String, Ipv4Addr)> = HashSet::new();

    unsafe {
        let mut ifap: *mut libc::ifaddrs = std::ptr::null_mut();
        if libc::getifaddrs(&mut ifap) != 0 || ifap.is_null() {
            return out;
        }
        let mut cur = ifap;
        while !cur.is_null() {
            let ifa = &*cur;
            let flags = ifa.ifa_flags as i32;
            let is_up = (flags & libc::IFF_UP) != 0;
            let is_loopback = (flags & libc::IFF_LOOPBACK) != 0;
            let is_p2p = (flags & libc::IFF_POINTOPOINT) != 0;

            if is_up
                && !is_loopback
                && !is_p2p
                && !ifa.ifa_addr.is_null()
                && (*ifa.ifa_addr).sa_family as i32 == libc::AF_INET
            {
                let name = if ifa.ifa_name.is_null() {
                    String::new()
                } else {
                    std::ffi::CStr::from_ptr(ifa.ifa_name)
                        .to_string_lossy()
                        .into_owned()
                };

                if !name.is_empty() && !is_virtual_or_tunnel_ifname(&name) {
                    let addr = &*(ifa.ifa_addr as *const libc::sockaddr_in);
                    let ip = Ipv4Addr::from(u32::from_be(addr.sin_addr.s_addr));
                    if !ip.is_unspecified() && !ip.is_loopback() && !ip.is_multicast() {
                        let netmask = if !ifa.ifa_netmask.is_null()
                            && (*ifa.ifa_netmask).sa_family as i32 == libc::AF_INET
                        {
                            let m = &*(ifa.ifa_netmask as *const libc::sockaddr_in);
                            Ipv4Addr::from(u32::from_be(m.sin_addr.s_addr))
                        } else {
                            Ipv4Addr::UNSPECIFIED
                        };

                        let mut broadcast = None;
                        if (flags & libc::IFF_BROADCAST) != 0 && !ifa.ifa_ifu.is_null() {
                            let broad = &*(ifa.ifa_ifu as *const libc::sockaddr_in);
                            if broad.sin_family as i32 == libc::AF_INET {
                                let b = Ipv4Addr::from(u32::from_be(broad.sin_addr.s_addr));
                                if !b.is_unspecified() && !b.is_loopback() {
                                    broadcast = Some(b);
                                }
                            }
                        }
                        if broadcast.is_none() {
                            let mask_u32 = u32::from(netmask);
                            if mask_u32 != 0 && mask_u32 != u32::MAX {
                                let b = Ipv4Addr::from(u32::from(ip) | !mask_u32);
                                if !b.is_unspecified() && !b.is_loopback() {
                                    broadcast = Some(b);
                                }
                            }
                        }

                        if seen.insert((name.clone(), ip)) {
                            out.push(LanInterfaceV4 {
                                name,
                                ip,
                                netmask,
                                broadcast,
                            });
                        }
                    }
                }
            }
            cur = ifa.ifa_next;
        }
        libc::freeifaddrs(ifap);
    }

    out.sort_by(|a, b| a.name.cmp(&b.name).then(a.ip.cmp(&b.ip)));
    out
}

/// Find the active physical LAN interface whose subnet contains `target`.
pub fn find_lan_interface_for_ipv4(target: Ipv4Addr) -> Option<LanInterfaceV4> {
    collect_lan_interfaces_v4()
        .into_iter()
        .find(|iface| iface.contains_ipv4(target))
}

/// Enumerate active non-loopback LAN IPv4 interface broadcast addresses via POSIX `getifaddrs`.
pub fn collect_ipv4_broadcast_targets() -> Vec<Ipv4Addr> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for iface in collect_lan_interfaces_v4() {
        if let Some(bcast) = iface.broadcast
            && seen.insert(bcast)
        {
            out.push(bcast);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::Platform;
    use tempfile::TempDir;

    #[tokio::test]
    async fn signed_beacons_discover_peers_over_udp() {
        let dir_a = TempDir::new().unwrap();
        let dir_b = TempDir::new().unwrap();
        let id_a = Arc::new(DeviceIdentity::generate("LinuxHost", Platform::Linux).unwrap());
        let id_b = Arc::new(DeviceIdentity::generate("PhoneHost", Platform::Android).unwrap());
        let trust_a = TrustRegistry::load_in_dir(dir_a.path()).unwrap();
        let trust_b = TrustRegistry::load_in_dir(dir_b.path()).unwrap();

        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        let (handle_b, join_b) = DiscoveryHandle::start(
            id_b.clone(),
            trust_b,
            DiscoveryConfig {
                discovery_port: 0,
                tls_port: 45902,
                announce_interval: Duration::from_secs(60),
                stale_ttl_ms: 15_000,
                extra_targets: vec![],
            },
            shutdown_rx.clone(),
        )
        .await
        .unwrap();

        let b_port = handle_b.local_addr().port();
        let b_loopback = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, b_port));

        let (handle_a, join_a) = DiscoveryHandle::start(
            id_a.clone(),
            trust_a,
            DiscoveryConfig {
                discovery_port: 0,
                tls_port: 45901,
                announce_interval: Duration::from_secs(60),
                stale_ttl_ms: 15_000,
                extra_targets: vec![b_loopback],
            },
            shutdown_rx,
        )
        .await
        .unwrap();

        let a_port = handle_a.local_addr().port();
        let a_loopback = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, a_port));
        // Point B back at A manually for ephemeral test port via udp packet simulation?
        // Actually B receives A's announcement right away! Let's trigger A -> B and verify B sees A.
        let mut rx_b = handle_b.subscribe();
        handle_a.announce_now().await;

        tokio::time::timeout(Duration::from_secs(2), async {
            while handle_b.get_peer(&id_a.device_id).is_none() {
                let _ = rx_b.changed().await;
            }
        })
        .await
        .expect("B should discover A");

        let seen_a = handle_b.get_peer(&id_a.device_id).unwrap();
        assert_eq!(seen_a.device_name, "LinuxHost");
        assert_eq!(seen_a.tls_port, 45901);
        assert!(!seen_a.is_trusted);
        let _ = a_loopback;

        let _ = shutdown_tx.send(true);
        let _ = join_a.await;
        let _ = join_b.await;
    }

    #[tokio::test]
    async fn evicts_stale_device_id_on_same_endpoint() {
        let dir_host = TempDir::new().unwrap();
        let id_host = Arc::new(DeviceIdentity::generate("LinuxHost", Platform::Linux).unwrap());
        let id_old = DeviceIdentity::generate("PhoneOld", Platform::Android).unwrap();
        let id_new = DeviceIdentity::generate("PhoneNew", Platform::Android).unwrap();
        let trust = TrustRegistry::load_in_dir(dir_host.path()).unwrap();

        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let (handle, join) = DiscoveryHandle::start(
            id_host,
            trust,
            DiscoveryConfig {
                discovery_port: 0,
                tls_port: 45910,
                announce_interval: Duration::from_secs(60),
                stale_ttl_ms: 15_000,
                extra_targets: vec![],
            },
            shutdown_rx,
        )
        .await
        .unwrap();

        let src = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(10, 20, 30, 40), 45876));
        let b1 = serde_json::to_vec(&id_old.sign_beacon(45877, now_timestamp_ms()).unwrap()).unwrap();
        assert!(handle_incoming_packet(&handle, &b1, src));
        assert!(handle.get_peer(&id_old.device_id).is_some());

        let b2 = serde_json::to_vec(&id_new.sign_beacon(45877, now_timestamp_ms()).unwrap()).unwrap();
        assert!(handle_incoming_packet(&handle, &b2, src));
        assert!(handle.get_peer(&id_old.device_id).is_none());
        assert_eq!(handle.list_peers().len(), 1);
        assert_eq!(handle.list_peers()[0].device_id, id_new.device_id);

        if let Some(local_ip) = collect_local_ipv4_addrs().into_iter().next() {
            let local_src = SocketAddr::V4(SocketAddrV4::new(local_ip, 45876));
            let b3 = serde_json::to_vec(&id_old.sign_beacon(45878, now_timestamp_ms()).unwrap()).unwrap();
            assert!(!handle_incoming_packet(&handle, &b3, local_src));
        }

        let _ = shutdown_tx.send(true);
        let _ = join.await;
    }

    #[test]
    fn lan_interface_filters_tunnels_and_matches_subnet() {
        assert!(is_virtual_or_tunnel_ifname("lo"));
        assert!(is_virtual_or_tunnel_ifname("tun11"));
        assert!(is_virtual_or_tunnel_ifname("tailscale0"));
        assert!(is_virtual_or_tunnel_ifname("rmnet_data0"));
        assert!(is_virtual_or_tunnel_ifname("br-d8f4ac855081"));
        assert!(is_virtual_or_tunnel_ifname("docker0"));

        assert!(!is_virtual_or_tunnel_ifname("wlan0"));
        assert!(!is_virtual_or_tunnel_ifname("enp131s0"));
        assert!(!is_virtual_or_tunnel_ifname("wlp3s0"));

        let wlan = LanInterfaceV4 {
            name: "wlan0".to_string(),
            ip: Ipv4Addr::new(192, 168, 1, 201),
            netmask: Ipv4Addr::new(255, 255, 255, 0),
            broadcast: Some(Ipv4Addr::new(192, 168, 1, 255)),
        };
        assert!(wlan.contains_ipv4(Ipv4Addr::new(192, 168, 1, 133)));
        assert!(!wlan.contains_ipv4(Ipv4Addr::new(192, 168, 2, 133)));
        assert!(!wlan.contains_ipv4(Ipv4Addr::new(100, 70, 151, 96)));
    }
}
