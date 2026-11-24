//! Standardized error codes and error envelopes shared across Linux and Android.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Machine-readable error codes returned by mTLS HTTP APIs, local IPC, and CLI diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    UnsupportedProtocol,
    InvalidMessage,
    InvalidEncoding,
    UnsupportedMime,
    MimeSignatureMismatch,
    CorruptedImage,
    ImageTooLarge,
    TextTooLarge,
    EmptyPayload,
    SizeMismatch,
    ChecksumMismatch,
    Base64DecodeFailed,
    ClipboardEmpty,
    ClipboardExcluded,
    ClipboardReadFailed,
    ClipboardWriteFailed,
    ImageReadDenied,
    ImageNotFound,
    Unauthorized,
    UntrustedPeer,
    PairingFailed,
    PairingNotFound,
    PairingRejected,
    SyncPaused,
    PayloadTooLarge,
    TransferFailed,
    TransferCancelled,
    NetworkUnreachable,
    RequestTimeout,
    InternalError,
}

impl ErrorCode {
    pub const fn exit_code(self) -> i32 {
        match self {
            Self::ClipboardEmpty => 10,
            Self::ClipboardExcluded => 11,
            Self::SyncPaused => 12,
            Self::Unauthorized | Self::UntrustedPeer => 20,
            Self::NetworkUnreachable => 21,
            Self::RequestTimeout => 22,
            Self::PairingFailed | Self::PairingNotFound | Self::PairingRejected => 23,
            Self::ImageTooLarge | Self::TextTooLarge | Self::PayloadTooLarge => 30,
            Self::UnsupportedMime
            | Self::MimeSignatureMismatch
            | Self::CorruptedImage
            | Self::InvalidEncoding
            | Self::InvalidMessage
            | Self::UnsupportedProtocol
            | Self::SizeMismatch
            | Self::ChecksumMismatch
            | Self::Base64DecodeFailed
            | Self::EmptyPayload => 31,
            Self::ImageReadDenied => 40,
            Self::ImageNotFound => 41,
            Self::ClipboardReadFailed => 42,
            Self::ClipboardWriteFailed => 43,
            Self::TransferFailed | Self::TransferCancelled => 44,
            Self::InternalError => 50,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::UnsupportedProtocol => "UNSUPPORTED_PROTOCOL",
            Self::InvalidMessage => "INVALID_MESSAGE",
            Self::InvalidEncoding => "INVALID_ENCODING",
            Self::UnsupportedMime => "UNSUPPORTED_MIME",
            Self::MimeSignatureMismatch => "MIME_SIGNATURE_MISMATCH",
            Self::CorruptedImage => "CORRUPTED_IMAGE",
            Self::ImageTooLarge => "IMAGE_TOO_LARGE",
            Self::TextTooLarge => "TEXT_TOO_LARGE",
            Self::EmptyPayload => "EMPTY_PAYLOAD",
            Self::SizeMismatch => "SIZE_MISMATCH",
            Self::ChecksumMismatch => "CHECKSUM_MISMATCH",
            Self::Base64DecodeFailed => "BASE64_DECODE_FAILED",
            Self::ClipboardEmpty => "CLIPBOARD_EMPTY",
            Self::ClipboardExcluded => "CLIPBOARD_EXCLUDED",
            Self::ClipboardReadFailed => "CLIPBOARD_READ_FAILED",
            Self::ClipboardWriteFailed => "CLIPBOARD_WRITE_FAILED",
            Self::ImageReadDenied => "IMAGE_READ_DENIED",
            Self::ImageNotFound => "IMAGE_NOT_FOUND",
            Self::Unauthorized => "UNAUTHORIZED",
            Self::UntrustedPeer => "UNTRUSTED_PEER",
            Self::PairingFailed => "PAIRING_FAILED",
            Self::PairingNotFound => "PAIRING_NOT_FOUND",
            Self::PairingRejected => "PAIRING_REJECTED",
            Self::SyncPaused => "SYNC_PAUSED",
            Self::PayloadTooLarge => "PAYLOAD_TOO_LARGE",
            Self::TransferFailed => "TRANSFER_FAILED",
            Self::TransferCancelled => "TRANSFER_CANCELLED",
            Self::NetworkUnreachable => "NETWORK_UNREACHABLE",
            Self::RequestTimeout => "REQUEST_TIMEOUT",
            Self::InternalError => "INTERNAL_ERROR",
        }
    }
}

/// Core protocol/codec validation errors. Never includes raw clipboard content.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CoreError {
    #[error("unsupported protocol version: {0}")]
    UnsupportedProtocol(u32),

    #[error("invalid message field `{field}`: {reason}")]
    InvalidMessage { field: &'static str, reason: String },

    #[error("invalid encoding `{encoding}` for kind `{kind}`")]
    InvalidEncoding { kind: String, encoding: String },

    #[error("unsupported MIME type: {0}")]
    UnsupportedMime(String),

    #[error("MIME `{declared}` does not match detected signature `{detected}`")]
    MimeSignatureMismatch { declared: String, detected: String },

    #[error("corrupted or truncated image payload: {0}")]
    CorruptedImage(String),

    #[error("image size {actual} bytes exceeds maximum {max} bytes")]
    ImageTooLarge { actual: usize, max: usize },

    #[error("text size {actual} bytes exceeds maximum {max} bytes")]
    TextTooLarge { actual: usize, max: usize },

    #[error("clipboard payload must not be empty")]
    EmptyPayload,

    #[error("declared size {declared} does not match actual size {actual}")]
    SizeMismatch { declared: usize, actual: usize },

    #[error("BLAKE3 checksum mismatch")]
    ChecksumMismatch,

    #[error("base64 decode failed: {0}")]
    Base64DecodeFailed(String),
}

impl CoreError {
    pub const fn code(&self) -> ErrorCode {
        match self {
            Self::UnsupportedProtocol(_) => ErrorCode::UnsupportedProtocol,
            Self::InvalidMessage { .. } => ErrorCode::InvalidMessage,
            Self::InvalidEncoding { .. } => ErrorCode::InvalidEncoding,
            Self::UnsupportedMime(_) => ErrorCode::UnsupportedMime,
            Self::MimeSignatureMismatch { .. } => ErrorCode::MimeSignatureMismatch,
            Self::CorruptedImage(_) => ErrorCode::CorruptedImage,
            Self::ImageTooLarge { .. } => ErrorCode::ImageTooLarge,
            Self::TextTooLarge { .. } => ErrorCode::TextTooLarge,
            Self::EmptyPayload => ErrorCode::EmptyPayload,
            Self::SizeMismatch { .. } => ErrorCode::SizeMismatch,
            Self::ChecksumMismatch => ErrorCode::ChecksumMismatch,
            Self::Base64DecodeFailed(_) => ErrorCode::Base64DecodeFailed,
        }
    }
}

/// Canonical JSON error envelope returned by HTTP APIs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiErrorBody {
    pub ok: bool,
    pub code: ErrorCode,
    pub message: String,
}

impl ApiErrorBody {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            ok: false,
            code,
            message: message.into(),
        }
    }
}
