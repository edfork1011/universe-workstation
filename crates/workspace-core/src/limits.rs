//! Protocol, framing, clipboard, and transfer limits for Workspace (寰宇工作台).

/// Current Workspace network protocol version.
pub const PROTOCOL_VERSION: u32 = 1;

/// Default UDP LAN discovery port.
pub const DEFAULT_DISCOVERY_PORT: u16 = 45876;

/// Default mTLS HTTPS service port (shared by Linux & Android).
pub const DEFAULT_TLS_PORT: u16 = 45877;

/// Default Android localhost control IPC port (`127.0.0.1` only, UID-authenticated).
pub const DEFAULT_ANDROID_LOCAL_PORT: u16 = 45878;

/// IPv4 multicast group used for LAN discovery beacons.
pub const DISCOVERY_MULTICAST_V4: &str = "239.255.77.88";

/// Hard upper bound for raw clipboard image bytes: 20 MiB (20,971,520 bytes).
pub const MAX_IMAGE_BYTES: usize = 20 * 1024 * 1024;

/// Maximum canonical Base64 character length for a 20 MiB image.
pub const MAX_IMAGE_BASE64_LEN: usize = MAX_IMAGE_BYTES.div_ceil(3) * 4;

/// Upper bound for UTF-8 text clipboard payloads: 1 MiB (1,048,576 bytes).
pub const MAX_TEXT_BYTES: usize = 1024 * 1024;

/// Maximum HTTP JSON request body accepted by clipboard/pair endpoints.
pub const MAX_HTTP_BODY_BYTES: usize = 32 * 1024 * 1024;

/// Maximum length of `message_id`.
pub const MAX_MESSAGE_ID_LEN: usize = 128;

/// Maximum length of `origin` / `device_id`.
pub const MAX_ORIGIN_LEN: usize = 64;

/// Canonical MIME type used for plain UTF-8 text messages.
pub const DEFAULT_TEXT_MIME: &str = "text/plain;charset=utf-8";

/// Default subfolder under `Downloads/` for received LAN files.
pub const RECEIVED_SUBDIR_NAME: &str = "寰宇工作台";

/// Format a byte count into a concise human-readable unit (`B`, `KiB`, `MiB`, `GiB`).
#[expect(
    clippy::cast_precision_loss,
    reason = "Human-readable byte formatting only"
)]
pub fn format_human_size(bytes: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = KIB * 1024;
    const GIB: u64 = MIB * 1024;
    if bytes >= GIB {
        format!("{:.2} GiB", (bytes as f64) / (GIB as f64))
    } else if bytes >= MIB {
        format!("{:.2} MiB", (bytes as f64) / (MIB as f64))
    } else if bytes >= KIB {
        format!("{:.1} KiB", (bytes as f64) / (KIB as f64))
    } else {
        format!("{bytes} B")
    }
}
