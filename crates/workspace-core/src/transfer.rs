//! Filename sanitization, percent-encoding, and streaming atomic file receiver.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use thiserror::Error;
use tokio::fs::{self, File, OpenOptions};
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

use crate::checksum::constant_time_eq;

static DEST_RENAME_LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug, Error)]
pub enum TransferError {
    #[error("file transfer I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("declared size mismatch: expected {expected} bytes, received {actual} bytes")]
    SizeMismatch { expected: u64, actual: u64 },
    #[error("file BLAKE3 checksum mismatch")]
    ChecksumMismatch,
    #[error("invalid expected checksum format")]
    InvalidChecksum,
    #[error("invalid transfer metadata: {0}")]
    InvalidMetadata(String),
}

/// Sanitize an untrusted remote filename against path traversal and filesystem hazards.
pub fn sanitize_filename(raw: &str) -> String {
    let last_segment = raw
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("")
        .chars()
        .filter(|c| !c.is_control() && *c != '\0')
        .collect::<String>();

    let trimmed = last_segment.trim().trim_matches('.');
    let candidate = if trimmed.is_empty() || last_segment.trim() == "." || last_segment.trim() == ".." {
        if last_segment.trim().starts_with('.')
            && last_segment.trim() != "."
            && last_segment.trim() != ".."
        {
            // Keep extensionless dotfile name stripped of leading dots safely.
            let stripped = last_segment.trim().trim_start_matches('.');
            if stripped.is_empty() {
                "unnamed_file".to_string()
            } else {
                stripped.to_string()
            }
        } else {
            "unnamed_file".to_string()
        }
    } else {
        last_segment.trim().to_string()
    };

    truncate_utf8_preserving_ext(&candidate, 180)
}

fn truncate_utf8_preserving_ext(name: &str, max_bytes: usize) -> String {
    if name.len() <= max_bytes {
        return name.to_string();
    }
    if let Some((stem, ext)) = split_stem_ext(name)
        && ext.len() + 2 < max_bytes
    {
        let budget = max_bytes - ext.len() - 1;
        let cut = floor_char_boundary(stem, budget);
        let prefix = &stem[..cut];
        if !prefix.is_empty() {
            return format!("{prefix}.{ext}");
        }
    }
    let cut = floor_char_boundary(name, max_bytes);
    let out = &name[..cut];
    if out.is_empty() {
        "unnamed_file".to_string()
    } else {
        out.to_string()
    }
}

fn floor_char_boundary(s: &str, max_bytes: usize) -> usize {
    let mut idx = max_bytes.min(s.len());
    while idx > 0 && !s.is_char_boundary(idx) {
        idx -= 1;
    }
    idx
}

fn split_stem_ext(name: &str) -> Option<(&str, &str)> {
    let dot = name.rfind('.')?;
    if dot == 0 || dot + 1 >= name.len() {
        return None;
    }
    let ext = &name[dot + 1..];
    if ext.len() > 16 {
        return None;
    }
    Some((&name[..dot], ext))
}

/// Choose a non-colliding path in `dest_dir`, using `"stem (1).ext"` on collisions.
pub fn unique_destination_path(dest_dir: &Path, safe_name: &str) -> PathBuf {
    let clean = sanitize_filename(safe_name);
    let candidate = dest_dir.join(&clean);
    if !candidate.exists() {
        return candidate;
    }

    let (stem, ext) = split_stem_ext(&clean)
        .map(|(s, e)| (s, Some(e)))
        .unwrap_or((&clean, None));
    for idx in 1..10_000usize {
        let alt = match ext {
            Some(ext) => format!("{stem} ({idx}).{ext}"),
            None => format!("{stem} ({idx})"),
        };
        let path = dest_dir.join(alt);
        if !path.exists() {
            return path;
        }
    }
    dest_dir.join(format!("{clean}.{}", Uuid::new_v4()))
}

/// Percent-encode arbitrary UTF-8 strings safely for HTTP headers (`X-Workspace-File-Name`).
pub fn percent_encode_utf8(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.as_bytes() {
        match *byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char);
            }
            b => {
                let _ = write!(out, "%{b:02X}");
            }
        }
    }
    out
}

/// Decode percent-encoded UTF-8 header values.
pub fn percent_decode_utf8(input: &str) -> Result<String, TransferError> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if i + 2 >= bytes.len() {
                return Err(TransferError::InvalidMetadata(
                    "truncated percent-escape in filename header".into(),
                ));
            }
            let hi = from_hex_digit(bytes[i + 1])?;
            let lo = from_hex_digit(bytes[i + 2])?;
            out.push((hi << 4) | lo);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out)
        .map_err(|e| TransferError::InvalidMetadata(format!("non-UTF-8 filename: {e}")))
}

fn from_hex_digit(b: u8) -> Result<u8, TransferError> {
    match b {
        b'0'..=b'9' => Ok(b - b'0'),
        b'a'..=b'f' => Ok(b - b'a' + 10),
        b'A'..=b'F' => Ok(b - b'A' + 10),
        _ => Err(TransferError::InvalidMetadata(
            "invalid hex digit in percent-encoded filename".into(),
        )),
    }
}

#[derive(Debug, Clone)]
pub struct ReceivedFile {
    pub path: PathBuf,
    pub file_name: String,
    pub size: u64,
    pub blake3_hex: String,
}

/// Streaming atomic file receiver with incremental BLAKE3 hashing and collision-safe commit.
pub struct FileReceiver {
    dest_dir: PathBuf,
    safe_name: String,
    part_path: PathBuf,
    file: Option<File>,
    hasher: blake3::Hasher,
    bytes_written: u64,
    declared_size: Option<u64>,
    expected_blake3_hex: Option<String>,
    committed: bool,
}

impl FileReceiver {
    /// Open a temporary `.part` file inside `dest_dir` ready to receive streamed chunks.
    pub async fn open(
        dest_dir: &Path,
        requested_name: &str,
        declared_size: Option<u64>,
        expected_blake3_hex: Option<String>,
    ) -> Result<Self, TransferError> {
        if let Some(ref hex) = expected_blake3_hex
            && (hex.len() != 64 || !hex.bytes().all(|b| b.is_ascii_hexdigit()))
        {
            return Err(TransferError::InvalidChecksum);
        }

        fs::create_dir_all(dest_dir).await?;
        let safe_name = sanitize_filename(requested_name);
        let part_path = dest_dir.join(format!(".{safe_name}.{}.part", Uuid::new_v4()));

        let file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&part_path)
            .await?;

        Ok(Self {
            dest_dir: dest_dir.to_path_buf(),
            safe_name,
            part_path,
            file: Some(file),
            hasher: blake3::Hasher::new(),
            bytes_written: 0,
            declared_size,
            expected_blake3_hex,
            committed: false,
        })
    }

    pub fn bytes_written(&self) -> u64 {
        self.bytes_written
    }

    /// Write one received byte chunk, checking size limits and updating incremental BLAKE3 state.
    pub async fn write_chunk(&mut self, chunk: &[u8]) -> Result<(), TransferError> {
        if chunk.is_empty() {
            return Ok(());
        }
        let next_total = self.bytes_written.saturating_add(chunk.len() as u64);
        if let Some(expected) = self.declared_size
            && next_total > expected
        {
            return Err(TransferError::SizeMismatch {
                expected,
                actual: next_total,
            });
        }

        if let Some(file) = self.file.as_mut() {
            file.write_all(chunk).await?;
        }
        self.hasher.update(chunk);
        self.bytes_written = next_total;
        Ok(())
    }

    /// Validate final size & BLAKE3 digest, sync to disk, and atomically rename to a unique path.
    pub async fn finish(mut self) -> Result<ReceivedFile, TransferError> {
        if let Some(expected) = self.declared_size
            && self.bytes_written != expected
        {
            return Err(TransferError::SizeMismatch {
                expected,
                actual: self.bytes_written,
            });
        }

        let actual_hex = self.hasher.finalize().to_hex().to_string();
        if let Some(ref expected_hex) = self.expected_blake3_hex {
            let expected_lower = expected_hex.to_ascii_lowercase();
            if !constant_time_eq(actual_hex.as_bytes(), expected_lower.as_bytes()) {
                return Err(TransferError::ChecksumMismatch);
            }
        }

        if let Some(mut file) = self.file.take() {
            file.flush().await?;
            file.sync_all().await?;
        }

        let dest_dir = self.dest_dir.clone();
        let safe_name = self.safe_name.clone();
        let part_path = self.part_path.clone();

        let final_path = tokio::task::spawn_blocking(move || -> std::io::Result<PathBuf> {
            let _guard = DEST_RENAME_LOCK
                .lock()
                .expect("destination rename lock poisoned");
            let dest = unique_destination_path(&dest_dir, &safe_name);
            std::fs::rename(&part_path, &dest)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                // Best-effort: Android FUSE storage may return EPERM/ENOTSUP on chmod.
                let _ = std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o644));
            }
            Ok(dest)
        })
        .await
        .map_err(|e| TransferError::Io(std::io::Error::other(e.to_string())))??;

        self.committed = true;
        let final_name = final_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(&self.safe_name)
            .to_string();

        Ok(ReceivedFile {
            path: final_path,
            file_name: final_name,
            size: self.bytes_written,
            blake3_hex: actual_hex,
        })
    }
}

impl Drop for FileReceiver {
    fn drop(&mut self) {
        if !self.committed {
            drop(self.file.take());
            let _ = std::fs::remove_file(&self.part_path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::checksum::compute_blake3_hex;
    use tempfile::TempDir;

    #[test]
    fn sanitize_blocks_traversal_and_preserves_unicode() {
        assert_eq!(sanitize_filename("../../etc/passwd"), "passwd");
        assert_eq!(sanitize_filename("..\\..\\Windows\\system.ini"), "system.ini");
        assert_eq!(sanitize_filename(".."), "unnamed_file");
        assert_eq!(sanitize_filename("测试 文档 (1).pdf"), "测试 文档 (1).pdf");
    }

    #[test]
    fn percent_roundtrip_handles_cjk_and_spaces() {
        let orig = "照片 2026-11-23 寰宇工作台.png";
        let enc = percent_encode_utf8(orig);
        assert!(enc.is_ascii());
        let dec = percent_decode_utf8(&enc).unwrap();
        assert_eq!(dec, orig);
    }

    #[tokio::test]
    async fn file_receiver_streams_verifies_and_dedups_names() {
        let dir = TempDir::new().unwrap();
        let data = b"hello workspace file payload";
        let hash = compute_blake3_hex(data);

        let mut rx1 = FileReceiver::open(
            dir.path(),
            "../evil/report.txt",
            Some(data.len() as u64),
            Some(hash.clone()),
        )
        .await
        .unwrap();
        rx1.write_chunk(&data[..10]).await.unwrap();
        rx1.write_chunk(&data[10..]).await.unwrap();
        let done1 = rx1.finish().await.unwrap();
        assert_eq!(done1.file_name, "report.txt");
        assert_eq!(std::fs::read(&done1.path).unwrap(), data);

        let mut rx2 = FileReceiver::open(
            dir.path(),
            "report.txt",
            Some(data.len() as u64),
            Some(hash),
        )
        .await
        .unwrap();
        rx2.write_chunk(data).await.unwrap();
        let done2 = rx2.finish().await.unwrap();
        assert_eq!(done2.file_name, "report (1).txt");
    }
}
