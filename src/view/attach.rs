//! What an attachment must be before it is held for sending: any file, at most `MAX_FILES`, none larger than the host takes. An image's
//! type is read from its bytes (never from a name). The host checks again when it sends (see `attachments`); this refuses early, with a
//! reason the person can read.
use crate::attachments::{MAX_FILES, MAX_FILE_BYTES};
use crate::claude::protocol::IMAGE_TYPES;

/// Most files in one message.
pub const MAX_IMAGES: usize = MAX_FILES;

/// The media type the bytes say they are, for the types the host sends.
pub fn sniff(bytes: &[u8]) -> Option<&'static str> {
    let kind = if bytes.starts_with(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]) {
        "image/png"
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        "image/jpeg"
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        "image/gif"
    } else if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        "image/webp"
    } else {
        return None;
    };
    IMAGE_TYPES.contains(&kind).then_some(kind)
}

/// Whether one more file of `len` decoded bytes may be held when `have` are already. The Err is an i18n key.
pub fn check_add(have: usize, len: usize) -> Result<(), &'static str> {
    if have >= MAX_FILES {
        return Err("att_too_many");
    }
    if len > MAX_FILE_BYTES {
        return Err("att_too_big");
    }
    Ok(())
}

/// A size for a person: bytes, KB or MB with one decimal.
pub fn human_size(n: u64) -> String {
    if n < 1024 {
        format!("{n} B")
    } else if n < 1024 * 1024 {
        format!("{:.1} KB", n as f64 / 1024.0)
    } else {
        format!("{:.1} MB", n as f64 / 1024.0 / 1024.0)
    }
}

/// The short label of a file's tile: its extension in capitals (at most four letters), or FILE.
pub fn ext_label(name: &str) -> String {
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() && !ext.is_empty() && ext.chars().all(|c| c.is_ascii_alphanumeric()) => ext.chars().take(4).collect::<String>().to_uppercase(),
        _ => "FILE".into(),
    }
}

/// The size the host computes from base64 text (the same arithmetic as `Session::send`).
pub fn decoded_len(b64: &str) -> usize {
    b64.len() / 4 * 3
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_type_comes_from_the_bytes() {
        assert_eq!(sniff(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0]), Some("image/png"));
        assert_eq!(sniff(&[0xFF, 0xD8, 0xFF, 0xE0]), Some("image/jpeg"));
        assert_eq!(sniff(b"GIF89a....."), Some("image/gif"));
        assert_eq!(sniff(b"RIFF\x10\0\0\0WEBPVP8 "), Some("image/webp"));
        for not in [&b"RIFF\x10\0\0\0WAVEfmt "[..], b"%PDF-1.7", b"<svg xmlns", b"", b"\x89PNG"] {
            assert_eq!(sniff(not), None, "{not:?}");
        }
    }

    #[test]
    fn sizes_and_labels_read_well() {
        assert_eq!(human_size(7), "7 B");
        assert_eq!(human_size(2048), "2.0 KB");
        assert_eq!(human_size(5 * 1024 * 1024), "5.0 MB");
        assert_eq!(ext_label("report.final.pdf"), "PDF");
        assert_eq!(ext_label("archive.tar.gzip"), "GZIP");
        assert_eq!(ext_label("archive.longext"), "LONG");
        assert_eq!(ext_label("Makefile"), "FILE");
        assert_eq!(ext_label(".bashrc"), "FILE");
        assert_eq!(ext_label("a."), "FILE");
    }

    #[test]
    fn count_and_size_are_limited_like_the_host_does() {
        assert_eq!(check_add(0, 10), Ok(()));
        assert_eq!(check_add(MAX_FILES - 1, MAX_FILE_BYTES), Ok(()));
        assert_eq!(check_add(MAX_FILES - 1, MAX_FILE_BYTES + 1), Err("att_too_big"));
        assert_eq!(check_add(MAX_FILES, 10), Err("att_too_many"));
        // The host's arithmetic on base64 text: exactly the limit passes, a little over does not.
        let at = "A".repeat(MAX_FILE_BYTES / 3 * 4);
        assert!(decoded_len(&at) <= MAX_FILE_BYTES);
        assert!(decoded_len(&"A".repeat(MAX_FILE_BYTES / 3 * 4 + 8)) > MAX_FILE_BYTES);
    }
}
