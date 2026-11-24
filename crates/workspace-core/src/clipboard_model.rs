//! Strict lossless clipboard codec & message model compatible with ClipBridge Protocol v1.
//!
//! Core invariant: `kind` and `data` are strictly separated. A `Text` message containing
//! Base64 characters is NEVER guessed or coerced into an image.

use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::checksum::{compute_blake3_hex, verify_blake3_hex};
use crate::error::CoreError;
use crate::limits::{
    DEFAULT_TEXT_MIME, MAX_IMAGE_BASE64_LEN, MAX_IMAGE_BYTES, MAX_MESSAGE_ID_LEN, MAX_ORIGIN_LEN,
    MAX_TEXT_BYTES, PROTOCOL_VERSION,
};

/// Image formats explicitly supported for lossless clipboard sync.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ImageFormat {
    Png,
    Jpeg,
    Webp,
}

impl ImageFormat {
    pub fn from_mime(mime: &str) -> Option<Self> {
        match mime.trim().to_ascii_lowercase().as_str() {
            "image/png" => Some(Self::Png),
            "image/jpeg" | "image/jpg" => Some(Self::Jpeg),
            "image/webp" => Some(Self::Webp),
            _ => None,
        }
    }

    pub const fn as_mime(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::Webp => "image/webp",
        }
    }

    pub const fn file_extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
            Self::Webp => "webp",
        }
    }
}

pub fn encode_base64(bytes: &[u8]) -> String {
    STANDARD.encode(bytes)
}

pub fn decode_image_base64(encoded: &str) -> Result<Vec<u8>, CoreError> {
    if encoded.is_empty() {
        return Err(CoreError::EmptyPayload);
    }
    if encoded.len() > MAX_IMAGE_BASE64_LEN {
        let approx_bytes = (encoded.len() / 4) * 3;
        return Err(CoreError::ImageTooLarge {
            actual: approx_bytes.max(MAX_IMAGE_BYTES + 1),
            max: MAX_IMAGE_BYTES,
        });
    }
    let decoded = STANDARD
        .decode(encoded)
        .map_err(|e| CoreError::Base64DecodeFailed(e.to_string()))?;

    if decoded.is_empty() {
        return Err(CoreError::EmptyPayload);
    }
    if decoded.len() > MAX_IMAGE_BYTES {
        return Err(CoreError::ImageTooLarge {
            actual: decoded.len(),
            max: MAX_IMAGE_BYTES,
        });
    }
    Ok(decoded)
}

fn sniff_magic_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(b"\xFF\xD8\xFF") {
        Some("image/jpeg")
    } else if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.starts_with(b"BM") {
        Some("image/bmp")
    } else if bytes.len() >= 12 && &bytes[4..8] == b"ftyp" {
        let brand = &bytes[8..12];
        if brand == b"avif" || brand == b"avis" {
            Some("image/avif")
        } else if brand == b"heic" || brand == b"heix" || brand == b"mif1" {
            Some("image/heic")
        } else {
            Some("application/octet-stream")
        }
    } else {
        None
    }
}

pub fn validate_image_bytes(declared_mime: &str, bytes: &[u8]) -> Result<ImageFormat, CoreError> {
    if bytes.is_empty() {
        return Err(CoreError::EmptyPayload);
    }
    if bytes.len() > MAX_IMAGE_BYTES {
        return Err(CoreError::ImageTooLarge {
            actual: bytes.len(),
            max: MAX_IMAGE_BYTES,
        });
    }

    let expected_format = ImageFormat::from_mime(declared_mime)
        .ok_or_else(|| CoreError::UnsupportedMime(declared_mime.to_string()))?;

    match sniff_magic_mime(bytes) {
        Some(detected_mime) if detected_mime != expected_format.as_mime() => {
            return Err(CoreError::MimeSignatureMismatch {
                declared: expected_format.as_mime().to_string(),
                detected: detected_mime.to_string(),
            });
        }
        None => {
            return Err(CoreError::CorruptedImage(
                "missing or unrecognized image magic header".to_string(),
            ));
        }
        Some(_) => {}
    }

    match expected_format {
        ImageFormat::Png => validate_png_structure(bytes)?,
        ImageFormat::Jpeg => validate_jpeg_structure(bytes)?,
        ImageFormat::Webp => validate_webp_structure(bytes)?,
    }

    Ok(expected_format)
}

pub fn detect_and_validate_image(bytes: &[u8]) -> Result<ImageFormat, CoreError> {
    if bytes.is_empty() {
        return Err(CoreError::EmptyPayload);
    }
    if bytes.len() > MAX_IMAGE_BYTES {
        return Err(CoreError::ImageTooLarge {
            actual: bytes.len(),
            max: MAX_IMAGE_BYTES,
        });
    }
    let mime = sniff_magic_mime(bytes).ok_or_else(|| {
        CoreError::CorruptedImage("missing or unrecognized image magic header".to_string())
    })?;
    let format =
        ImageFormat::from_mime(mime).ok_or_else(|| CoreError::UnsupportedMime(mime.to_string()))?;
    validate_image_bytes(format.as_mime(), bytes)
}

fn crc32_ieee(chunk_type: &[u8; 4], data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &byte in chunk_type.iter().chain(data.iter()) {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            if (crc & 1) != 0 {
                crc = (crc >> 1) ^ 0xEDB8_8320;
            } else {
                crc >>= 1;
            }
        }
    }
    !crc
}

fn validate_png_structure(bytes: &[u8]) -> Result<(), CoreError> {
    const PNG_SIG: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
    if bytes.len() < 45 || &bytes[0..8] != PNG_SIG {
        return Err(CoreError::CorruptedImage("truncated PNG header".to_string()));
    }

    let mut pos = 8usize;
    let mut saw_ihdr = false;
    let mut saw_idat = false;
    let mut saw_iend = false;

    while pos < bytes.len() {
        if saw_iend {
            return Err(CoreError::CorruptedImage(
                "trailing bytes after PNG IEND chunk".to_string(),
            ));
        }
        if pos + 12 > bytes.len() {
            return Err(CoreError::CorruptedImage(
                "truncated PNG chunk header".to_string(),
            ));
        }
        let len = u32::from_be_bytes([bytes[pos], bytes[pos + 1], bytes[pos + 2], bytes[pos + 3]])
            as usize;
        let chunk_type: [u8; 4] = [bytes[pos + 4], bytes[pos + 5], bytes[pos + 6], bytes[pos + 7]];
        let data_start = pos + 8;
        let data_end = data_start
            .checked_add(len)
            .ok_or_else(|| CoreError::CorruptedImage("PNG chunk length overflow".to_string()))?;
        let crc_end = data_end
            .checked_add(4)
            .ok_or_else(|| CoreError::CorruptedImage("PNG chunk CRC overflow".to_string()))?;

        if crc_end > bytes.len() {
            return Err(CoreError::CorruptedImage("truncated PNG chunk data".to_string()));
        }

        let chunk_data = &bytes[data_start..data_end];
        let expected_crc = u32::from_be_bytes([
            bytes[data_end],
            bytes[data_end + 1],
            bytes[data_end + 2],
            bytes[data_end + 3],
        ]);
        if crc32_ieee(&chunk_type, chunk_data) != expected_crc {
            return Err(CoreError::CorruptedImage(format!(
                "CRC mismatch in PNG chunk `{}`",
                String::from_utf8_lossy(&chunk_type)
            )));
        }

        if !saw_ihdr {
            if &chunk_type != b"IHDR" || len != 13 {
                return Err(CoreError::CorruptedImage(
                    "PNG must start with a 13-byte IHDR chunk".to_string(),
                ));
            }
            let width = u32::from_be_bytes([
                chunk_data[0],
                chunk_data[1],
                chunk_data[2],
                chunk_data[3],
            ]);
            let height = u32::from_be_bytes([
                chunk_data[4],
                chunk_data[5],
                chunk_data[6],
                chunk_data[7],
            ]);
            if width == 0 || height == 0 {
                return Err(CoreError::CorruptedImage(
                    "PNG dimensions must be non-zero".to_string(),
                ));
            }
            saw_ihdr = true;
        } else if &chunk_type == b"IDAT" {
            if !chunk_data.is_empty() {
                saw_idat = true;
            }
        } else if &chunk_type == b"IEND" {
            if len != 0 {
                return Err(CoreError::CorruptedImage(
                    "PNG IEND chunk must have zero length".to_string(),
                ));
            }
            saw_iend = true;
        }

        pos = crc_end;
    }

    if !saw_ihdr || !saw_idat || !saw_iend {
        return Err(CoreError::CorruptedImage(
            "incomplete PNG stream (missing IHDR, IDAT, or IEND)".to_string(),
        ));
    }
    Ok(())
}

fn validate_jpeg_structure(bytes: &[u8]) -> Result<(), CoreError> {
    if bytes.len() < 20 || !bytes.starts_with(b"\xFF\xD8\xFF") {
        return Err(CoreError::CorruptedImage("invalid JPEG SOI header".to_string()));
    }

    let mut pos = 2usize;
    let mut saw_sof = false;
    let mut saw_sos = false;

    while pos < bytes.len() {
        if bytes[pos] != 0xFF {
            return Err(CoreError::CorruptedImage(
                "unexpected byte outside JPEG marker segment".to_string(),
            ));
        }
        while pos < bytes.len() && bytes[pos] == 0xFF {
            pos += 1;
        }
        if pos >= bytes.len() {
            return Err(CoreError::CorruptedImage("truncated JPEG marker".to_string()));
        }
        let marker = bytes[pos];
        pos += 1;

        match marker {
            0x00 => {
                return Err(CoreError::CorruptedImage(
                    "invalid stuffed 0xFF00 marker outside entropy scan".to_string(),
                ));
            }
            0xD9 => {
                return Err(CoreError::CorruptedImage(
                    "premature JPEG EOI before scan data".to_string(),
                ));
            }
            0xD8 | 0x01 | 0xD0..=0xD7 => continue,
            _ => {}
        }

        if pos + 2 > bytes.len() {
            return Err(CoreError::CorruptedImage(
                "truncated JPEG segment length".to_string(),
            ));
        }
        let seg_len = u16::from_be_bytes([bytes[pos], bytes[pos + 1]]) as usize;
        if seg_len < 2 || pos + seg_len > bytes.len() {
            return Err(CoreError::CorruptedImage(
                "JPEG segment length out of bounds".to_string(),
            ));
        }

        let is_sof = matches!(
            marker,
            0xC0..=0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF
        );
        if is_sof {
            if seg_len < 8 {
                return Err(CoreError::CorruptedImage(
                    "truncated JPEG SOF segment".to_string(),
                ));
            }
            let height = u16::from_be_bytes([bytes[pos + 3], bytes[pos + 4]]);
            let width = u16::from_be_bytes([bytes[pos + 5], bytes[pos + 6]]);
            if width == 0 || height == 0 {
                return Err(CoreError::CorruptedImage(
                    "JPEG dimensions must be non-zero".to_string(),
                ));
            }
            saw_sof = true;
        }

        pos += seg_len;

        if marker == 0xDA {
            saw_sos = true;
            break;
        }
    }

    if !saw_sof || !saw_sos {
        return Err(CoreError::CorruptedImage(
            "JPEG missing SOF or SOS marker".to_string(),
        ));
    }

    if pos + 2 > bytes.len() {
        return Err(CoreError::CorruptedImage(
            "truncated JPEG entropy scan".to_string(),
        ));
    }

    let trimmed_end = bytes.len();
    if trimmed_end < 2 || bytes[trimmed_end - 2..trimmed_end] != [0xFF, 0xD9] {
        return Err(CoreError::CorruptedImage(
            "missing JPEG EOI (0xFFD9) trailer".to_string(),
        ));
    }

    Ok(())
}

fn validate_webp_structure(bytes: &[u8]) -> Result<(), CoreError> {
    if bytes.len() < 20 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WEBP" {
        return Err(CoreError::CorruptedImage("invalid WebP RIFF header".to_string()));
    }

    let riff_size = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]) as usize;
    if riff_size < 12 || riff_size.checked_add(8) != Some(bytes.len()) {
        return Err(CoreError::CorruptedImage(
            "WebP RIFF size does not match file length".to_string(),
        ));
    }

    let mut pos = 12usize;
    let mut first_chunk = true;

    while pos < bytes.len() {
        if pos + 8 > bytes.len() {
            return Err(CoreError::CorruptedImage(
                "truncated WebP sub-chunk header".to_string(),
            ));
        }
        let fourcc = &bytes[pos..pos + 4];
        let chunk_len =
            u32::from_le_bytes([bytes[pos + 4], bytes[pos + 5], bytes[pos + 6], bytes[pos + 7]])
                as usize;
        let data_start = pos + 8;
        let data_end = data_start
            .checked_add(chunk_len)
            .ok_or_else(|| CoreError::CorruptedImage("WebP chunk length overflow".to_string()))?;

        if data_end > bytes.len() {
            return Err(CoreError::CorruptedImage(
                "WebP chunk extends past RIFF container".to_string(),
            ));
        }
        let chunk_data = &bytes[data_start..data_end];

        if first_chunk {
            match fourcc {
                b"VP8 " => {
                    if chunk_data.len() < 10
                        || (chunk_data[0] & 1) != 0
                        || chunk_data[3..6] != [0x9D, 0x01, 0x2A]
                    {
                        return Err(CoreError::CorruptedImage(
                            "invalid WebP VP8 keyframe header".to_string(),
                        ));
                    }
                }
                b"VP8L" => {
                    if chunk_data.len() < 5 || chunk_data[0] != 0x2F {
                        return Err(CoreError::CorruptedImage(
                            "invalid WebP VP8L header".to_string(),
                        ));
                    }
                }
                b"VP8X" => {
                    if chunk_data.len() != 10 {
                        return Err(CoreError::CorruptedImage(
                            "WebP VP8X chunk must be 10 bytes".to_string(),
                        ));
                    }
                }
                _ => {
                    return Err(CoreError::CorruptedImage(
                        "WebP must start with VP8, VP8L, or VP8X chunk".to_string(),
                    ));
                }
            }
            first_chunk = false;
        }

        let padded_len = chunk_len + (chunk_len & 1);
        pos = data_start.saturating_add(padded_len).min(bytes.len());
        if (chunk_len & 1) == 1 && data_end == bytes.len() {
            return Err(CoreError::CorruptedImage(
                "missing RIFF odd-length padding byte in WebP".to_string(),
            ));
        }
    }

    Ok(())
}

/// Logical clipboard payload category.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ClipKind {
    Text,
    Image,
}

impl ClipKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Image => "image",
        }
    }
}

/// Wire encoding of `ClipboardMessage::data`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PayloadEncoding {
    Utf8,
    Base64,
}

impl PayloadEncoding {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Utf8 => "utf8",
            Self::Base64 => "base64",
        }
    }
}

/// Clipboard Protocol v1 message envelope.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClipboardMessage {
    pub protocol: u32,
    pub message_id: String,
    pub origin: String,
    pub sequence: u64,
    pub timestamp_ms: u64,
    pub kind: ClipKind,
    pub mime: String,
    pub encoding: PayloadEncoding,
    pub size: usize,
    pub checksum: String,
    pub data: String,
}

impl fmt::Debug for ClipboardMessage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClipboardMessage")
            .field("protocol", &self.protocol)
            .field("message_id", &self.message_id)
            .field("origin", &self.origin)
            .field("sequence", &self.sequence)
            .field("timestamp_ms", &self.timestamp_ms)
            .field("kind", &self.kind)
            .field("mime", &self.mime)
            .field("encoding", &self.encoding)
            .field("size", &self.size)
            .field("checksum", &self.checksum)
            .field("data", &format!("<redacted {} chars>", self.data.len()))
            .finish()
    }
}

/// Validated, decoded clipboard payload ready for writing to the OS clipboard.
#[derive(Clone, PartialEq, Eq)]
pub enum ValidatedPayload {
    Text {
        mime: String,
        text: String,
    },
    Image {
        format: ImageFormat,
        mime: String,
        bytes: Vec<u8>,
    },
}

impl fmt::Debug for ValidatedPayload {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Text { mime, text } => f
                .debug_struct("ValidatedPayload::Text")
                .field("mime", mime)
                .field("bytes_len", &text.len())
                .finish(),
            Self::Image {
                format,
                mime,
                bytes,
            } => f
                .debug_struct("ValidatedPayload::Image")
                .field("format", format)
                .field("mime", mime)
                .field("bytes_len", &bytes.len())
                .finish(),
        }
    }
}

impl ValidatedPayload {
    pub fn kind(&self) -> ClipKind {
        match self {
            Self::Text { .. } => ClipKind::Text,
            Self::Image { .. } => ClipKind::Image,
        }
    }

    pub fn mime(&self) -> &str {
        match self {
            Self::Text { mime, .. } | Self::Image { mime, .. } => mime,
        }
    }

    pub fn raw_bytes(&self) -> &[u8] {
        match self {
            Self::Text { text, .. } => text.as_bytes(),
            Self::Image { bytes, .. } => bytes,
        }
    }
}

/// Metadata summary of a `ClipboardMessage` without sensitive payload content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotMetadata {
    pub message_id: String,
    pub origin: String,
    pub sequence: u64,
    pub timestamp_ms: u64,
    pub kind: ClipKind,
    pub mime: String,
    pub size: usize,
    pub checksum: String,
}

impl From<&ClipboardMessage> for SnapshotMetadata {
    fn from(msg: &ClipboardMessage) -> Self {
        Self {
            message_id: msg.message_id.clone(),
            origin: msg.origin.clone(),
            sequence: msg.sequence,
            timestamp_ms: msg.timestamp_ms,
            kind: msg.kind,
            mime: msg.mime.clone(),
            size: msg.size,
            checksum: msg.checksum.clone(),
        }
    }
}

pub fn now_timestamp_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn is_supported_text_mime(mime: &str) -> bool {
    let lower = mime.trim().to_ascii_lowercase();
    lower == "text/plain"
        || lower == "text/plain;charset=utf-8"
        || lower == "text/plain; charset=utf-8"
}

fn validate_identifier(field: &'static str, value: &str, max_len: usize) -> Result<(), CoreError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(CoreError::InvalidMessage {
            field,
            reason: "must not be empty".to_string(),
        });
    }
    if value.len() > max_len {
        return Err(CoreError::InvalidMessage {
            field,
            reason: format!("exceeds maximum length of {max_len} characters"),
        });
    }
    if value.chars().any(char::is_control) {
        return Err(CoreError::InvalidMessage {
            field,
            reason: "must not contain control characters".to_string(),
        });
    }
    Ok(())
}

impl ClipboardMessage {
    pub fn new_text(
        origin: impl Into<String>,
        sequence: u64,
        text: impl Into<String>,
    ) -> Result<Self, CoreError> {
        let origin = origin.into();
        validate_identifier("origin", &origin, MAX_ORIGIN_LEN)?;

        let text = text.into();
        let raw_bytes = text.as_bytes();
        if raw_bytes.is_empty() {
            return Err(CoreError::EmptyPayload);
        }
        if raw_bytes.len() > MAX_TEXT_BYTES {
            return Err(CoreError::TextTooLarge {
                actual: raw_bytes.len(),
                max: MAX_TEXT_BYTES,
            });
        }

        let checksum = compute_blake3_hex(raw_bytes);
        Ok(Self {
            protocol: PROTOCOL_VERSION,
            message_id: Uuid::new_v4().to_string(),
            origin,
            sequence,
            timestamp_ms: now_timestamp_ms(),
            kind: ClipKind::Text,
            mime: DEFAULT_TEXT_MIME.to_string(),
            encoding: PayloadEncoding::Utf8,
            size: raw_bytes.len(),
            checksum,
            data: text,
        })
    }

    pub fn new_image(
        origin: impl Into<String>,
        sequence: u64,
        mime: &str,
        raw_bytes: &[u8],
    ) -> Result<Self, CoreError> {
        let origin = origin.into();
        validate_identifier("origin", &origin, MAX_ORIGIN_LEN)?;

        if raw_bytes.is_empty() {
            return Err(CoreError::EmptyPayload);
        }
        if raw_bytes.len() > MAX_IMAGE_BYTES {
            return Err(CoreError::ImageTooLarge {
                actual: raw_bytes.len(),
                max: MAX_IMAGE_BYTES,
            });
        }

        let format = validate_image_bytes(mime, raw_bytes)?;
        let checksum = compute_blake3_hex(raw_bytes);
        let data = encode_base64(raw_bytes);

        Ok(Self {
            protocol: PROTOCOL_VERSION,
            message_id: Uuid::new_v4().to_string(),
            origin,
            sequence,
            timestamp_ms: now_timestamp_ms(),
            kind: ClipKind::Image,
            mime: format.as_mime().to_string(),
            encoding: PayloadEncoding::Base64,
            size: raw_bytes.len(),
            checksum,
            data,
        })
    }

    pub fn validate_and_decode(&self) -> Result<ValidatedPayload, CoreError> {
        if self.protocol != PROTOCOL_VERSION {
            return Err(CoreError::UnsupportedProtocol(self.protocol));
        }
        validate_identifier("message_id", &self.message_id, MAX_MESSAGE_ID_LEN)?;
        validate_identifier("origin", &self.origin, MAX_ORIGIN_LEN)?;

        if self.size == 0 {
            return Err(CoreError::EmptyPayload);
        }

        match self.kind {
            ClipKind::Text => {
                if self.encoding != PayloadEncoding::Utf8 {
                    return Err(CoreError::InvalidEncoding {
                        kind: self.kind.as_str().to_string(),
                        encoding: self.encoding.as_str().to_string(),
                    });
                }
                if !is_supported_text_mime(&self.mime) {
                    return Err(CoreError::UnsupportedMime(self.mime.clone()));
                }
                let raw_bytes = self.data.as_bytes();
                if raw_bytes.is_empty() {
                    return Err(CoreError::EmptyPayload);
                }
                if self.size > MAX_TEXT_BYTES || raw_bytes.len() > MAX_TEXT_BYTES {
                    return Err(CoreError::TextTooLarge {
                        actual: raw_bytes.len().max(self.size),
                        max: MAX_TEXT_BYTES,
                    });
                }
                if raw_bytes.len() != self.size {
                    return Err(CoreError::SizeMismatch {
                        declared: self.size,
                        actual: raw_bytes.len(),
                    });
                }
                verify_blake3_hex(raw_bytes, &self.checksum)?;

                Ok(ValidatedPayload::Text {
                    mime: DEFAULT_TEXT_MIME.to_string(),
                    text: self.data.clone(),
                })
            }
            ClipKind::Image => {
                if self.encoding != PayloadEncoding::Base64 {
                    return Err(CoreError::InvalidEncoding {
                        kind: self.kind.as_str().to_string(),
                        encoding: self.encoding.as_str().to_string(),
                    });
                }
                if self.size > MAX_IMAGE_BYTES {
                    return Err(CoreError::ImageTooLarge {
                        actual: self.size,
                        max: MAX_IMAGE_BYTES,
                    });
                }
                let expected_format = ImageFormat::from_mime(&self.mime)
                    .ok_or_else(|| CoreError::UnsupportedMime(self.mime.clone()))?;

                let decoded = decode_image_base64(&self.data)?;
                if decoded.len() != self.size {
                    return Err(CoreError::SizeMismatch {
                        declared: self.size,
                        actual: decoded.len(),
                    });
                }
                verify_blake3_hex(&decoded, &self.checksum)?;
                let format = validate_image_bytes(expected_format.as_mime(), &decoded)?;

                Ok(ValidatedPayload::Image {
                    format,
                    mime: format.as_mime().to_string(),
                    bytes: decoded,
                })
            }
        }
    }
}
