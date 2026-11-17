//! What an image attachment must be before it is held for sending: its type is read from its bytes (never from a name), there are
//! at most five, and none is larger than the host forwards. The host checks again when it sends; this refuses early, with a reason
//! the person can read.
use crate::claude::protocol::{IMAGE_TYPES, MAX_IMAGE_BYTES};

/// Most images in one message.
pub const MAX_IMAGES: usize = 5;

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

/// Whether one more image of `len` decoded bytes may be held when `have` are already. The Err is an i18n key.
pub fn check_add(have: usize, len: usize, kind: Option<&str>) -> Result<(), &'static str> {
    if kind.is_none() {
        return Err("att_bad_type");
    }
    if have >= MAX_IMAGES {
        return Err("att_too_many");
    }
    if len > MAX_IMAGE_BYTES {
        return Err("att_too_big");
    }
    Ok(())
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
    fn count_and_size_are_limited_like_the_host_does() {
        assert_eq!(check_add(0, 10, Some("image/png")), Ok(()));
        assert_eq!(check_add(4, MAX_IMAGE_BYTES, Some("image/png")), Ok(()));
        assert_eq!(check_add(4, MAX_IMAGE_BYTES + 1, Some("image/png")), Err("att_too_big"));
        assert_eq!(check_add(5, 10, Some("image/png")), Err("att_too_many"));
        assert_eq!(check_add(0, 10, None), Err("att_bad_type"));
        // The host's arithmetic on base64 text: exactly 5 MiB passes, a little over does not.
        let at = "A".repeat(MAX_IMAGE_BYTES / 3 * 4);
        assert!(decoded_len(&at) <= MAX_IMAGE_BYTES);
        assert!(decoded_len(&"A".repeat(MAX_IMAGE_BYTES / 3 * 4 + 8)) > MAX_IMAGE_BYTES);
    }
}
