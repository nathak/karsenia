//! Files attached to the message being written: any file, chosen with a file picker, or an image pasted from the clipboard; held (at
//! most ten, each at most what the host takes) with a small thumbnail for an image or a tile with the extension for another file, and
//! handed to `send_text` when the message is sent. What a file is, is read from its bytes by the host, never from a name. Bytes are
//! dropped as soon as they are encoded: what is kept is the base64 the host needs and a 128-pixel thumbnail.
use std::cell::RefCell;
use std::io::Cursor;
use std::sync::Arc;

use base64::Engine;
use slint::{ComponentHandle, Image as SlintImage, ModelRc, Rgba8Pixel, SharedPixelBuffer, VecModel};
use tenrec::attachments::MAX_FILE_BYTES;
use tenrec::claude::protocol::{Image, MAX_IMAGE_BYTES};
use tenrec::view::attach::{check_add, decoded_len, ext_label, sniff, MAX_IMAGES};
use tenrec::view::i18n::tr;

use crate::{App, AppWindow, Att, AttV};

/// One held image. The thumbnail is plain pixels (a Slint image cannot leave the UI thread, and a failed send hands these back).
pub struct Pending {
    media_type: &'static str,
    b64: String,
    thumb: Option<(u32, u32, Vec<u8>)>,
    label: String,
    name: Option<String>,
}

thread_local! {
    // The images attached in each chat that was left, put back when it is opened again.
    static STASH: RefCell<std::collections::HashMap<String, Vec<Pending>>> = RefCell::new(std::collections::HashMap::new());
    static HELD: RefCell<Vec<Pending>> = const { RefCell::new(Vec::new()) };
}

fn render(ui: &AppWindow) {
    let items: Vec<AttV> = HELD.with(|h| {
        h.borrow()
            .iter()
            .map(|p| AttV {
                thumb: p.thumb.as_ref().map(|(w, hgt, raw)| SlintImage::from_rgba8(SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(raw, *w, *hgt))).unwrap_or_default(),
                label: p.label.as_str().into(),
                name: p.name.clone().unwrap_or_default().into(),
                has_thumb: p.thumb.is_some(),
            })
            .collect()
    });
    ui.global::<Att>().set_items(ModelRc::from(std::rc::Rc::new(VecModel::from(items))));
    ui.global::<Att>().set_max(MAX_IMAGES as i32);
}

pub fn count() -> usize {
    HELD.with(|h| h.borrow().len())
}

/// The held images for sending; they are no longer held.
pub fn take(ui: &AppWindow) -> (Vec<Image>, Vec<Pending>) {
    let held: Vec<Pending> = HELD.with(|h| std::mem::take(&mut *h.borrow_mut()));
    render(ui);
    let images = held.iter().map(|p| Image { media_type: p.media_type.to_owned(), data: p.b64.clone(), name: p.name.clone() }).collect();
    (images, held)
}

/// A send that failed: the images come back, unless something was attached meanwhile.
pub fn restore(ui: &AppWindow, held: Vec<Pending>) {
    if HELD.with(|h| h.borrow().is_empty()) {
        HELD.with(|h| *h.borrow_mut() = held);
        render(ui);
    }
}

/// Another chat was opened: what is attached stays with the chat that was left, and the opened chat has its own.
pub fn switch(ui: &AppWindow, left: Option<&str>, to: &str) {
    let held: Vec<Pending> = HELD.with(|h| std::mem::take(&mut *h.borrow_mut()));
    STASH.with(|s| {
        let mut s = s.borrow_mut();
        if let Some(old) = left {
            if held.is_empty() {
                s.remove(old);
            } else {
                s.insert(old.to_owned(), held);
            }
        }
        HELD.with(|h| *h.borrow_mut() = s.remove(to).unwrap_or_default());
    });
    render(ui);
}

/// The chat is gone: so is what was attached to it.
pub fn forget(ui: &AppWindow, sid: &str) {
    STASH.with(|s| s.borrow_mut().remove(sid));
    clear(ui);
}

pub fn clear(ui: &AppWindow) {
    HELD.with(|h| h.borrow_mut().clear());
    render(ui);
}

/// Checked, encoded and given a thumbnail off the UI thread; added on it.
struct Ready {
    media_type: &'static str,
    b64: String,
    thumb: Option<(u32, u32, Vec<u8>)>,
    name: Option<String>,
    is_image: bool,
}

fn prepare(bytes: Vec<u8>, have: usize, name: Option<String>) -> Result<Ready, &'static str> {
    check_add(have, bytes.len())?;
    let kind = sniff(&bytes);
    // Not an image (or one with no decoder here): sent as it is, and shown as a tile with its extension.
    let media_type = kind.unwrap_or("application/octet-stream");
    let format = match media_type {
        "image/png" => Some(image::ImageFormat::Png),
        "image/jpeg" => Some(image::ImageFormat::Jpeg),
        _ => None,
    };
    let thumb = format.and_then(|f| image::load_from_memory_with_format(&bytes, f).ok()).map(|img| {
        let t = img.thumbnail(128, 128).to_rgba8();
        (t.width(), t.height(), t.into_raw())
    });
    Ok(Ready { media_type, b64: base64::engine::general_purpose::STANDARD.encode(&bytes), thumb, name, is_image: kind.is_some() })
}

fn add(ui: &AppWindow, app: &App, ready: Result<Ready, &'static str>) {
    match ready {
        Ok(r) => {
            // Checked again here: two images prepared at once can both have seen room for one.
            if count() >= MAX_IMAGES || decoded_len(&r.b64) > MAX_FILE_BYTES {
                crate::toast(ui, tr(app.lang, "att_too_many"));
                return;
            }
            let thumb = r.thumb;
            let label = match r.media_type {
                "image/gif" => "GIF".to_owned(),
                "image/webp" => "WEBP".to_owned(),
                _ if r.is_image => "IMG".to_owned(),
                _ => ext_label(r.name.as_deref().unwrap_or("")),
            };
            HELD.with(|h| h.borrow_mut().push(Pending { media_type: r.media_type, b64: r.b64, thumb, label, name: r.name }));
            render(ui);
        }
        Err(key) => crate::toast(ui, tr(app.lang, key)),
    }
}

/// Bytes from a file or the clipboard: prepared on a worker thread, then added.
/// `epoch` is the chat on screen when the picture was chosen or pasted: if another chat is on screen by the time it is ready, it is
/// dropped (it was meant for the chat that was left, possibly of another account or provider).
fn add_bytes(app: &Arc<App>, bytes: Vec<u8>, name: Option<String>, epoch: u64) {
    let have = count();
    let a = app.clone();
    tokio::spawn(async move {
        let ready = tokio::task::spawn_blocking(move || prepare(bytes, have, name)).await.unwrap_or(Err("att_read_fail"));
        let b = a.clone();
        let _ = a.weak.upgrade_in_event_loop(move |ui| {
            if b.current.lock().expect("current").epoch == epoch {
                add(&ui, &b, ready);
            }
        });
    });
}

/// RGBA from the clipboard as PNG, or JPEG when the PNG is too large for the host.
fn encode_clipboard(width: u32, height: u32, rgba: Vec<u8>) -> Result<Vec<u8>, &'static str> {
    let img = image::RgbaImage::from_raw(width, height, rgba).ok_or("att_read_fail")?;
    let mut png = Cursor::new(Vec::new());
    img.write_to(&mut png, image::ImageFormat::Png).map_err(|_| "att_read_fail")?;
    let png = png.into_inner();
    if png.len() <= MAX_IMAGE_BYTES {
        return Ok(png);
    }
    let rgb = image::DynamicImage::ImageRgba8(img).to_rgb8();
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 85).encode_image(&rgb).map_err(|_| "att_read_fail")?;
    if jpeg.len() <= MAX_IMAGE_BYTES {
        Ok(jpeg)
    } else {
        Err("att_too_big")
    }
}

pub fn wire(ui: &AppWindow, app: &Arc<App>) {
    let att = ui.global::<Att>();

    let (a, weak) = (app.clone(), ui.as_weak());
    att.on_pick(move || {
        let Some(ui) = weak.upgrade() else { return };
        if count() >= MAX_IMAGES {
            crate::toast(&ui, tr(a.lang, "att_too_many"));
            return;
        }
        let epoch = a.current.lock().expect("current").epoch;
        let handle = ui.window().window_handle();
        let picked = rfd::AsyncFileDialog::new().set_title(tr(a.lang, "att_add")).set_parent(&handle).pick_files();
        let (b, weak) = (a.clone(), ui.as_weak());
        let _ = slint::spawn_local(async move {
            let Some(files) = picked.await else { return };
            for file in files {
                // Looked at before it is read: a huge file is not loaded to be refused.
                let path = file.path().to_owned();
                let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
                if std::fs::metadata(&path).map(|m| m.len() as usize > MAX_FILE_BYTES).unwrap_or(false) {
                    if let Some(ui) = weak.upgrade() {
                        crate::toast(&ui, tr(b.lang, "att_too_big"));
                    }
                    continue;
                }
                match tokio::task::spawn_blocking(move || std::fs::read(path)).await {
                    Ok(Ok(bytes)) => add_bytes(&b, bytes, name, epoch),
                    _ => {
                        if let Some(ui) = weak.upgrade() {
                            crate::toast(&ui, tr(b.lang, "att_read_fail"));
                        }
                    }
                }
            }
        });
    });

    let weak = ui.as_weak();
    att.on_remove(move |i| {
        HELD.with(|h| {
            let mut h = h.borrow_mut();
            if (i as usize) < h.len() {
                h.remove(i as usize);
            }
        });
        if let Some(ui) = weak.upgrade() {
            render(&ui);
        }
    });

    // Ctrl+V in the composer: an image on the clipboard is attached; anything else is left to the text field. True when it was ours.
    let (a, weak) = (app.clone(), ui.as_weak());
    att.on_paste_image(move || {
        let Ok(mut clipboard) = arboard::Clipboard::new() else { return false };
        // Text first: a copy from a page carries both text and an image, and the person meant the text.
        if clipboard.get_text().is_ok_and(|t| !t.is_empty()) {
            return false;
        }
        let Ok(data) = clipboard.get_image() else { return false };
        let Some(ui) = weak.upgrade() else { return true };
        if count() >= MAX_IMAGES {
            crate::toast(&ui, tr(a.lang, "att_too_many"));
            return true;
        }
        let (width, height, rgba) = (data.width as u32, data.height as u32, data.bytes.into_owned());
        let epoch = a.current.lock().expect("current").epoch;
        let b = a.clone();
        tokio::spawn(async move {
            match tokio::task::spawn_blocking(move || encode_clipboard(width, height, rgba)).await.unwrap_or(Err("att_read_fail")) {
                Ok(bytes) => add_bytes(&b, bytes, None, epoch),
                Err(key) => {
                    let c = b.clone();
                    let _ = b.weak.upgrade_in_event_loop(move |ui| crate::toast(&ui, tr(c.lang, key)));
                }
            }
        });
        true
    });
}
