//! Files attached to a message. Any file may be attached; what is done with it is decided by its bytes, never by the media type or the
//! name the sender gave: an image goes to the model as an image, a PDF as a document, a small text file is put in the message, and
//! anything else (a binary, a big file) is saved in this app's own folder for the chat and the message names where. The checks come
//! first and write nothing; files are written only for a message that passed them.
use std::path::{Path, PathBuf};

use base64::Engine;
use serde_json::{json, Value};

use crate::claude::protocol::{Image, Part, MAX_IMAGE_BYTES};
use crate::view::attach::sniff;

/// Most files in one message.
pub const MAX_FILES: usize = 10;
/// The largest file, in decoded bytes.
pub const MAX_FILE_BYTES: usize = 25 * 1024 * 1024;
/// The largest PDF (the model's own limit is lower for some models; the answer then shows the refusal).
pub const MAX_PDF_BYTES: usize = 20 * 1024 * 1024;
/// Images and PDFs of one message together (they travel inside the message line).
pub const MAX_INLINE_BYTES: usize = 24 * 1024 * 1024;
/// All the files of one message together (decoded): with the base64 and the rest of the request this stays inside what the WebSocket takes.
pub const MAX_TOTAL_BYTES: usize = 28 * 1024 * 1024;
/// A text file is put in the message up to this size, and all of them together up to the second; beyond that they are saved.
pub const MAX_TEXT_BYTES: usize = 128 * 1024;
pub const MAX_TEXT_TOTAL: usize = 256 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    /// An image of this media type.
    Image(&'static str),
    Pdf,
    /// Valid UTF-8 without a NUL byte.
    Text,
    Binary,
}

pub fn classify(bytes: &[u8]) -> Class {
    if let Some(kind) = sniff(bytes) {
        return Class::Image(kind);
    }
    if bytes.starts_with(b"%PDF-") {
        return Class::Pdf;
    }
    if !bytes.contains(&0) && std::str::from_utf8(bytes).is_ok() {
        return Class::Text;
    }
    Class::Binary
}

/// A file that passed the checks.
#[derive(Debug)]
pub struct Checked {
    pub name: String,
    pub bytes: Vec<u8>,
    pub class: Class,
}

/// The size the host computes from base64 text.
fn decoded_len(b64: &str) -> usize {
    b64.len() / 4 * 3
}

/// A name that is safe to write under the folder of a chat: only the last part, no control characters or separators, no leading dot,
/// at most 100 characters; `file` when nothing is left.
pub fn safe_name(name: Option<&str>) -> String {
    let raw = name.unwrap_or("");
    let last = raw.rsplit(['/', '\\']).next().unwrap_or("");
    // Characters no file name may have on Windows go too (the host may be Windows whatever the sender runs on).
    let mut clean: String = last.chars().filter(|c| !c.is_control() && !":<>\"|?*".contains(*c)).take(100).collect();
    clean = clean.trim().trim_end_matches(['.', ' ']).to_owned();
    // Nothing but dots or underscores is no name.
    if clean.chars().all(|c| c == '.' || c == '_') {
        return "file".into();
    }
    if clean.starts_with('.') {
        clean.replace_range(..1, "_");
    }
    // A device name (`NUL`, `CON`, `COM1`, with or without an extension) is a device on Windows, not a file.
    let stem = clean.split('.').next().unwrap_or("").to_ascii_uppercase();
    let device = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL") || (stem.len() == 4 && (stem.starts_with("COM") || stem.starts_with("LPT")) && stem.as_bytes()[3].is_ascii_digit());
    if device {
        clean.insert(0, '_');
    }
    clean
}

/// Checks a message's files, in this order: how many, each one's encoded size, that it decodes, its kind, the limits of that kind, the
/// size of what travels inside the message, how much text goes in it. A container chat cannot see this app's folders, so it takes only
/// what goes in the message. The Err is a sentence for the person.
pub fn check(list: &[Image], container: bool) -> Result<Vec<Checked>, String> {
    if list.len() > MAX_FILES {
        return Err(format!("at most {MAX_FILES} attachments per message"));
    }
    if list.iter().any(|f| decoded_len(&f.data) > MAX_FILE_BYTES + 3) {
        return Err(format!("a file can be {} MB at most", MAX_FILE_BYTES / 1024 / 1024));
    }
    let mut out = Vec::with_capacity(list.len());
    let (mut inline, mut text_total, mut total) = (0usize, 0usize, 0usize);
    for (i, file) in list.iter().enumerate() {
        let bytes = base64::engine::general_purpose::STANDARD.decode(file.data.trim()).map_err(|_| "an attachment is not valid base64".to_owned())?;
        if bytes.len() > MAX_FILE_BYTES {
            return Err(format!("a file can be {} MB at most", MAX_FILE_BYTES / 1024 / 1024));
        }
        total += bytes.len();
        if total > MAX_TOTAL_BYTES {
            return Err(format!("the files of one message can be {} MB together", MAX_TOTAL_BYTES / 1024 / 1024));
        }
        let mut class = classify(&bytes);
        // An image or a PDF over what the model takes inside a message is kept as a file instead (a container chat cannot, below).
        if matches!(class, Class::Image(_)) && bytes.len() > MAX_IMAGE_BYTES || class == Class::Pdf && bytes.len() > MAX_PDF_BYTES {
            class = Class::Binary;
        }
        if matches!(class, Class::Image(_) | Class::Pdf) {
            inline += bytes.len();
            if inline > MAX_INLINE_BYTES {
                return Err("the images and PDFs of one message can be 24 MB together".into());
            }
        }
        if class == Class::Text {
            if bytes.len() > MAX_TEXT_BYTES || text_total + bytes.len() > MAX_TEXT_TOTAL {
                class = Class::Binary;
            } else {
                text_total += bytes.len();
            }
        }
        if container && class == Class::Binary {
            return Err("a container chat can take images up to 5 MB, PDFs up to 20 MB and small text files only".into());
        }
        let name = match file.name.as_deref() {
            Some(n) => safe_name(Some(n)),
            None => match class {
                Class::Image(kind) => format!("image-{}.{}", i + 1, kind.trim_start_matches("image/").replace("jpeg", "jpg")),
                Class::Pdf => format!("document-{}.pdf", i + 1),
                _ => format!("file-{}", i + 1),
            },
        };
        out.push(Checked { name, bytes, class });
    }
    Ok(out)
}

/// Where a chat's saved files are.
pub fn folder(dir: &Path, sid: &str) -> PathBuf {
    dir.join("attachments").join(sid)
}

/// Saves one file under the chat's folder, private, under a name that is not taken (`a.bin`, `a-1.bin`, ...).
pub fn store(dir: &Path, sid: &str, name: &str, bytes: &[u8]) -> std::io::Result<PathBuf> {
    let base = folder(dir, sid);
    crate::private::create_dir(&dir.join("attachments"))?;
    crate::private::create_dir(&base)?;
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    for n in 0..1000 {
        let candidate = if n == 0 { name.to_owned() } else { format!("{stem}-{n}{ext}") };
        let path = base.join(&candidate);
        match crate::private::write_bytes_new(&path, bytes) {
            Ok(()) => return Ok(path),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => {
                // Half written (the disk was full): not left behind.
                let _ = std::fs::remove_file(&path);
                return Err(e);
            }
        }
    }
    Err(std::io::Error::new(std::io::ErrorKind::AlreadyExists, "too many files with that name"))
}

/// What a message is made of, and what the transcript shows of it: the parts for `user_message`, the images (media type and data, for
/// thumbnails) and the other files (name, size, kind; never their content) for the `user` event. A file that cannot be saved ends the
/// whole thing, and what was already saved for this message is removed.
pub fn materialize(dir: &Path, sid: &str, checked: Vec<Checked>) -> Result<(Vec<Part>, Vec<Value>, Vec<Value>), String> {
    let (mut parts, mut images, mut files, mut saved) = (Vec::new(), Vec::new(), Vec::new(), Vec::<PathBuf>::new());
    let b64 = |bytes: &[u8]| base64::engine::general_purpose::STANDARD.encode(bytes);
    for file in checked {
        let size = file.bytes.len();
        match file.class {
            Class::Image(kind) => {
                let data = b64(&file.bytes);
                images.push(json!({"media_type": kind, "data": data}));
                parts.push(Part::Image(Image { media_type: kind.into(), data, name: Some(file.name) }));
            }
            Class::Pdf => {
                files.push(json!({"name": file.name, "size": size, "kind": "pdf"}));
                parts.push(Part::Pdf { name: file.name, data: b64(&file.bytes) });
            }
            Class::Text => {
                files.push(json!({"name": file.name, "size": size, "kind": "text"}));
                parts.push(Part::Text { name: file.name, body: String::from_utf8_lossy(&file.bytes).into_owned() });
            }
            Class::Binary => match store(dir, sid, &file.name, &file.bytes) {
                Ok(path) => {
                    saved.push(path.clone());
                    files.push(json!({"name": file.name, "size": size, "kind": "file"}));
                    parts.push(Part::Stored { name: file.name, path, size });
                }
                Err(error) => {
                    for path in saved {
                        let _ = std::fs::remove_file(path);
                    }
                    return Err(format!("could not save the attached file: {error}"));
                }
            },
        }
    }
    Ok((parts, images, files))
}

/// Removes everything saved for a chat (the chat was deleted).
pub fn remove_all(dir: &Path, sid: &str) {
    if sid.chars().all(|c| c.is_ascii_hexdigit()) && !sid.is_empty() {
        let _ = std::fs::remove_dir_all(folder(dir, sid));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(bytes: &[u8], name: Option<&str>) -> Image {
        Image { media_type: "application/octet-stream".into(), data: base64::engine::general_purpose::STANDARD.encode(bytes), name: name.map(str::to_owned) }
    }

    const PNG: &[u8] = &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0, 0];

    #[test]
    fn the_kind_comes_from_the_bytes() {
        assert_eq!(classify(PNG), Class::Image("image/png"));
        assert_eq!(classify(b"%PDF-1.7\n%\xe2\xe3"), Class::Pdf);
        assert_eq!(classify("hello 안녕\n".as_bytes()), Class::Text);
        assert_eq!(classify(b"a\0b"), Class::Binary);
        assert_eq!(classify(&[0xff, 0xfe, 0xfd]), Class::Binary);
        assert_eq!(classify(b""), Class::Text);
    }

    #[test]
    fn names_are_made_safe() {
        assert_eq!(safe_name(Some("../../etc/passwd")), "passwd");
        assert_eq!(safe_name(Some("C:\\Users\\a\\report.pdf")), "report.pdf");
        assert_eq!(safe_name(Some(".bashrc")), "_bashrc");
        assert_eq!(safe_name(Some("a\0b\n.txt")), "ab.txt");
        assert_eq!(safe_name(Some("")), "file");
        assert_eq!(safe_name(None), "file");
        assert_eq!(safe_name(Some("..")), "file");
        assert_eq!(safe_name(Some(&"x".repeat(300))).len(), 100);
        assert_eq!(safe_name(Some("a<b>|c?.txt")), "abc.txt");
        assert_eq!(safe_name(Some("report. ")), "report");
        assert_eq!(safe_name(Some("NUL.txt")), "_NUL.txt");
        assert_eq!(safe_name(Some("com1")), "_com1");
        assert_eq!(safe_name(Some("console.log")), "console.log");
    }

    #[test]
    fn the_checks_run_in_order_and_demote_big_text() {
        assert!(check(&(0..11).map(|_| file(b"a", None)).collect::<Vec<_>>(), false).unwrap_err().contains("at most 10"));
        assert!(check(&[Image { media_type: "x".into(), data: "!!!!".into(), name: None }], false).unwrap_err().contains("base64"));
        assert!(check(&[file(&vec![b'a'; MAX_FILE_BYTES + 1], None)], false).unwrap_err().contains("25 MB"));
        // An image over 5 MB (or a PDF over 20 MB) is kept as a file; a container chat cannot take that.
        let mut big_png = PNG.to_vec();
        big_png.resize(MAX_IMAGE_BYTES + 1, 0);
        assert_eq!(check(&[file(&big_png, Some("big.png"))], false).unwrap()[0].class, Class::Binary);
        assert!(check(&[file(&big_png, Some("big.png"))], true).unwrap_err().contains("container"));
        // Together they must stay under the total (two 20 MB files pass one by one, not together).
        assert!(check(&[file(&vec![b'a'; 20 * 1024 * 1024], None), file(&vec![0u8; 20 * 1024 * 1024], None)], false).unwrap_err().contains("28 MB together"));
        // Text up to the limit is text; over it, and over the total, it is saved like any file.
        let ok = check(&[file(b"hello", Some("n.txt")), file(&vec![b'a'; MAX_TEXT_BYTES], Some("big.txt")), file(&vec![b'b'; MAX_TEXT_BYTES], Some("big2.txt")), file(&vec![b'c'; MAX_TEXT_BYTES + 1], Some("huge.txt"))], false).unwrap();
        let kinds: Vec<Class> = ok.iter().map(|c| c.class).collect();
        assert_eq!(kinds, [Class::Text, Class::Text, Class::Binary, Class::Binary], "{kinds:?}");
        // A container chat takes no file that would have to be saved.
        assert!(check(&[file(b"\0\0", Some("a.bin"))], true).unwrap_err().contains("container"));
        assert!(check(&[file(b"text", Some("a.txt")), file(PNG, None)], true).is_ok());
        // A file with no name is given one by its kind.
        assert_eq!(check(&[file(PNG, None)], false).unwrap()[0].name, "image-1.png");
    }

    #[test]
    fn saved_files_are_private_unique_and_removed_with_the_chat() {
        let dir = tempfile::tempdir().unwrap();
        let a = store(dir.path(), "abc123", "a.bin", b"one").unwrap();
        let b = store(dir.path(), "abc123", "a.bin", b"two").unwrap();
        assert_eq!(a.file_name().unwrap(), "a.bin");
        assert_eq!(b.file_name().unwrap(), "a-1.bin");
        assert_eq!(std::fs::read(&a).unwrap(), b"one");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&a).unwrap().permissions().mode() & 0o777, 0o600);
            assert_eq!(std::fs::metadata(folder(dir.path(), "abc123")).unwrap().permissions().mode() & 0o777, 0o700);
        }
        remove_all(dir.path(), "abc123");
        assert!(!folder(dir.path(), "abc123").exists());
        // Not a chat id: nothing is removed.
        let keep = store(dir.path(), "def456", "k.bin", b"k").unwrap();
        remove_all(dir.path(), "../def456");
        assert!(keep.exists());
    }

    #[test]
    fn a_message_is_made_of_parts_and_the_event_has_no_content_of_files() {
        let dir = tempfile::tempdir().unwrap();
        let checked = check(&[file(PNG, Some("p.png")), file(b"%PDF-1.4 x", Some("d.pdf")), file(b"notes", Some("n.txt")), file(b"\0\x01", Some("z.bin"))], false).unwrap();
        let (parts, images, files) = materialize(dir.path(), "abc123", checked).unwrap();
        assert_eq!(parts.len(), 4);
        assert_eq!(images.len(), 1);
        assert_eq!(files.iter().map(|f| f["kind"].as_str().unwrap()).collect::<Vec<_>>(), ["pdf", "text", "file"]);
        assert!(!files.iter().any(|f| f.get("data").is_some()), "no content in the event");
        assert!(matches!(&parts[3], Part::Stored { path, size: 2, .. } if path.exists()));
    }
}
