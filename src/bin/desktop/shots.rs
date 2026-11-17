//! The pictures of the user messages in the transcript: a small thumbnail under each message (decoded off the UI thread, kept only
//! while the message is on screen) and the full-size view opened by a click. Only png and jpeg are drawn; the other types stay a
//! labelled tile (the message text says so). The bytes come from the transcript's rows, never from a file path.
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::io::Cursor;
use std::rc::Rc;
use std::sync::Arc;

use base64::Engine;
use slint::{ComponentHandle, Image as SlintImage, ModelRc, Rgba8Pixel, SharedPixelBuffer, VecModel};
use tenrec::claude::protocol::Image;
use tenrec::view::transcript::{Row, THUMB_TYPES};

use crate::{App, AppWindow, ChatImg};

/// Thumbnails are cut at twice the size they are shown (HiDPI); the full view is cut at this long side.
const THUMB: u32 = 192;
const FULL_MAX: u32 = 2560;

type Pixels = (u32, u32, Vec<u8>);

thread_local! {
    // Per message id (unique in the process): the pictures of a row that is on screen, their decoded thumbnails and the ones being decoded.
    static SRC: RefCell<HashMap<u32, Arc<[Image]>>> = RefCell::new(HashMap::new());
    static THUMBS: RefCell<HashMap<u32, Vec<ChatImg>>> = RefCell::new(HashMap::new());
    static PENDING: RefCell<HashSet<u32>> = RefCell::new(HashSet::new());
    // Which opening of the full view is current: a decode that finishes after the view was closed or reopened is dropped.
    static ZOOM: Cell<u64> = const { Cell::new(0) };
}

fn decode(image: &Image) -> Option<image::DynamicImage> {
    let format = match image.media_type.as_str() {
        "image/png" => image::ImageFormat::Png,
        "image/jpeg" => image::ImageFormat::Jpeg,
        _ => return None,
    };
    let bytes = base64::engine::general_purpose::STANDARD.decode(&image.data).ok()?;
    let mut reader = image::ImageReader::with_format(Cursor::new(bytes), format);
    // A picture that claims a huge size is refused before its buffer is made.
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    limits.max_alloc = Some(256 * 1024 * 1024);
    reader.limits(limits);
    reader.decode().ok()
}

fn raw(img: image::DynamicImage) -> Pixels {
    let rgba = img.to_rgba8();
    (rgba.width(), rgba.height(), rgba.into_raw())
}

/// A thumbnail as plain pixels (a Slint image cannot leave the UI thread).
fn thumb(image: &Image) -> Option<Pixels> {
    decode(image).map(|i| raw(i.thumbnail(THUMB, THUMB)))
}

/// The picture for the full view, cut down when its long side passes `FULL_MAX`.
fn full(image: &Image) -> Option<Pixels> {
    let img = decode(image)?;
    let big = img.width().max(img.height());
    Some(raw(if big > FULL_MAX { img.resize(FULL_MAX, FULL_MAX, image::imageops::FilterType::Triangle) } else { img }))
}

fn slint_image((w, h, px): &Pixels) -> SlintImage {
    SlintImage::from_rgba8(SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(px, *w, *h))
}

fn label(media_type: &str) -> &'static str {
    match media_type {
        "image/gif" => "GIF",
        "image/webp" => "WEBP",
        _ => "IMG",
    }
}

/// What a row shows of its pictures: the decoded thumbnails, or a tile per picture while they are being decoded.
pub fn tiles(row: &Row) -> (i32, ModelRc<ChatImg>) {
    let Some(shots) = &row.images else { return (0, ModelRc::default()) };
    let tiles = THUMBS.with(|t| t.borrow().get(&shots.id).cloned()).unwrap_or_else(|| {
        shots.list.iter().map(|i| ChatImg { thumb: SlintImage::default(), label: if THUMB_TYPES.contains(&i.media_type.as_str()) { "…".into() } else { label(&i.media_type).into() }, has_thumb: false }).collect()
    });
    (shots.id as i32, ModelRc::from(Rc::new(VecModel::from(tiles))))
}

/// Called after the rows on screen changed: forgets what is no longer shown and starts decoding what is new (one worker for the
/// whole batch, so opening a long history does not decode everything at once).
pub fn sync(rows: &[Row], app: &Arc<App>) {
    let shown: HashMap<u32, Arc<[Image]>> = rows.iter().filter_map(|r| r.images.as_ref()).map(|s| (s.id, s.list.clone())).collect();
    SRC.with(|s| s.borrow_mut().retain(|id, _| shown.contains_key(id)));
    THUMBS.with(|t| t.borrow_mut().retain(|id, _| shown.contains_key(id)));
    PENDING.with(|p| p.borrow_mut().retain(|id| shown.contains_key(id)));
    let mut todo: Vec<(u32, Arc<[Image]>)> = Vec::new();
    for (id, list) in shown {
        SRC.with(|s| s.borrow_mut().insert(id, list.clone()));
        let known = THUMBS.with(|t| t.borrow().contains_key(&id)) || PENDING.with(|p| p.borrow().contains(&id));
        if !known {
            PENDING.with(|p| p.borrow_mut().insert(id));
            todo.push((id, list));
        }
    }
    if todo.is_empty() {
        return;
    }
    todo.sort_by_key(|(id, _)| *id);
    let a = app.clone();
    tokio::spawn(async move {
        let done = tokio::task::spawn_blocking(move || todo.into_iter().map(|(id, list)| (id, list.iter().map(|i| (label(&i.media_type), thumb(i))).collect::<Vec<_>>())).collect::<Vec<_>>()).await.unwrap_or_default();
        let _ = a.weak.upgrade_in_event_loop(move |_| {
            for (id, parts) in done {
                PENDING.with(|p| p.borrow_mut().remove(&id));
                // The chat was left meanwhile: nothing is kept for it.
                if !SRC.with(|s| s.borrow().contains_key(&id)) {
                    continue;
                }
                let tiles = parts.iter().map(|(l, px)| ChatImg { thumb: px.as_ref().map(slint_image).unwrap_or_default(), label: (*l).into(), has_thumb: px.is_some() }).collect();
                THUMBS.with(|t| t.borrow_mut().insert(id, tiles));
                crate::refresh_rows_of(id);
            }
        });
    });
}

/// A picture was clicked: its thumbnail shows at once, then the full picture replaces it.
fn open(ui: &AppWindow, app: &Arc<App>, id: i32, index: i32) {
    let id = id.max(0) as u32;
    let Some(image) = SRC.with(|s| s.borrow().get(&id).and_then(|l| l.get(index.max(0) as usize).cloned())) else { return };
    if !THUMB_TYPES.contains(&image.media_type.as_str()) {
        return;
    }
    let token = ZOOM.with(|z| {
        z.set(z.get() + 1);
        z.get()
    });
    if let Some(first) = THUMBS.with(|t| t.borrow().get(&id).and_then(|v| v.get(index as usize).map(|c| c.thumb.clone()))) {
        ui.set_zoom_image(first);
    }
    ui.set_zoom_loading(true);
    ui.set_zoom_open(true);
    let a = app.clone();
    tokio::spawn(async move {
        let px = tokio::task::spawn_blocking(move || full(&image)).await.unwrap_or(None);
        let _ = a.weak.upgrade_in_event_loop(move |ui| {
            if ZOOM.with(|z| z.get()) != token || !ui.get_zoom_open() {
                return;
            }
            if let Some(px) = px {
                ui.set_zoom_image(slint_image(&px));
            }
            ui.set_zoom_loading(false);
        });
    });
}

pub fn close(ui: &AppWindow) {
    ZOOM.with(|z| z.set(z.get() + 1));
    // The big buffer is let go.
    ui.set_zoom_image(SlintImage::default());
    ui.set_zoom_loading(false);
    ui.set_zoom_open(false);
}

/// Another chat (or none) is on screen: nothing is kept of the last one.
pub fn reset(ui: &AppWindow) {
    SRC.with(|s| s.borrow_mut().clear());
    THUMBS.with(|t| t.borrow_mut().clear());
    PENDING.with(|p| p.borrow_mut().clear());
    close(ui);
}

pub fn wire(ui: &AppWindow, app: &Arc<App>) {
    let (a, weak) = (app.clone(), ui.as_weak());
    ui.on_open_image(move |id, i| {
        if let Some(ui) = weak.upgrade() {
            open(&ui, &a, id, i);
        }
    });
    let weak = ui.as_weak();
    ui.on_zoom_close(move || {
        if let Some(ui) = weak.upgrade() {
            close(&ui);
            ui.invoke_refocus();
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encoded(format: image::ImageFormat, w: u32, h: u32) -> String {
        let img = image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(w, h, image::Rgb([200, 30, 30])));
        let mut out = Cursor::new(Vec::new());
        img.write_to(&mut out, format).unwrap();
        base64::engine::general_purpose::STANDARD.encode(out.into_inner())
    }
    fn img(media_type: &str, data: String) -> Image {
        Image { media_type: media_type.into(), data }
    }

    #[test]
    fn png_and_jpeg_get_a_thumbnail_that_keeps_the_shape() {
        for (format, kind) in [(image::ImageFormat::Png, "image/png"), (image::ImageFormat::Jpeg, "image/jpeg")] {
            let (w, h, px) = thumb(&img(kind, encoded(format, 300, 200))).unwrap();
            assert!(w.max(h) <= THUMB && w > h, "{w}x{h}");
            assert_eq!(px.len() as u32, w * h * 4);
        }
    }

    #[test]
    fn other_types_and_broken_data_give_nothing() {
        assert!(thumb(&img("image/gif", encoded(image::ImageFormat::Png, 10, 10))).is_none());
        assert!(thumb(&img("image/webp", "AAAA".into())).is_none());
        assert!(thumb(&img("image/png", "not base64 !!".into())).is_none());
        assert!(thumb(&img("image/png", "AAAA".into())).is_none());
    }

    #[test]
    fn the_full_view_is_cut_to_the_limit() {
        let (w, h, _) = full(&img("image/png", encoded(image::ImageFormat::Png, 4000, 100))).unwrap();
        assert!(w <= FULL_MAX && h >= 1 && w > h);
        let (w, h, _) = full(&img("image/png", encoded(image::ImageFormat::Png, 300, 200))).unwrap();
        assert_eq!((w, h), (300, 200));
    }
}
