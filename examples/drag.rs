//! Checks, headless (Slint's software renderer, no display), that a drag selects text of a chat paragraph and Ctrl+C copies it, also where
//! the paragraph is styled (bold, code) or holds a link, and that a click on a link still follows it:
//!   cargo run --features desktop --example drag -- <picture folder>
//! It uses the same `Rich` component as the chat rows. The exit status is 0 when everything holds.
#![allow(deprecated)] // the `slint!` macro asks for an edition: the app's own build sets none, and so does this
use std::cell::RefCell;
use std::rc::Rc;

use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Clipboard, Platform, PointerEventButton, WindowAdapter, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, PhysicalSize, Rgb8Pixel, StyledText};

slint::slint! {
    import { Rich, Theme } from "slint/base.slint";
    export component Probe inherits Window {
        width: 480px;
        height: 150px;
        in property <styled-text> first;
        in property <string> first-plain;
        in property <styled-text> second;
        in property <string> second-plain;
        out property <string> last-link;
        background: Theme.surface;
        VerticalLayout {
            padding: 10px;
            spacing: 14px;
            Rich { rich: root.first; plain: root.first-plain; size: 15px; fg: Theme.on-surface; }
            Rich { rich: root.second; plain: root.second-plain; size: 15px; fg: Theme.on-surface; link(u) => { root.last-link = u; } }
        }
    }
}

struct Headless {
    window: Rc<MinimalSoftwareWindow>,
    copied: Rc<RefCell<String>>,
}

impl Platform for Headless {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.window.clone())
    }
    fn set_clipboard_text(&self, text: &str, _clipboard: Clipboard) {
        *self.copied.borrow_mut() = text.to_owned();
    }
}

fn at(x: f32, y: f32) -> LogicalPosition {
    LogicalPosition::new(x, y)
}

fn drag(window: &MinimalSoftwareWindow, from: (f32, f32), to: (f32, f32)) {
    window.dispatch_event(WindowEvent::PointerMoved { position: at(from.0, from.1) });
    window.dispatch_event(WindowEvent::PointerPressed { position: at(from.0, from.1), button: PointerEventButton::Left });
    for step in 1..=8 {
        let t = step as f32 / 8.0;
        window.dispatch_event(WindowEvent::PointerMoved { position: at(from.0 + (to.0 - from.0) * t, from.1 + (to.1 - from.1) * t) });
    }
    window.dispatch_event(WindowEvent::PointerReleased { position: at(to.0, to.1), button: PointerEventButton::Left });
}

fn click(window: &MinimalSoftwareWindow, p: (f32, f32)) {
    window.dispatch_event(WindowEvent::PointerMoved { position: at(p.0, p.1) });
    window.dispatch_event(WindowEvent::PointerPressed { position: at(p.0, p.1), button: PointerEventButton::Left });
    window.dispatch_event(WindowEvent::PointerReleased { position: at(p.0, p.1), button: PointerEventButton::Left });
}

fn ctrl_c(window: &MinimalSoftwareWindow) {
    window.dispatch_event(WindowEvent::KeyPressed { text: slint::platform::Key::Control.into() });
    window.dispatch_event(WindowEvent::KeyPressed { text: "c".into() });
    window.dispatch_event(WindowEvent::KeyReleased { text: "c".into() });
    window.dispatch_event(WindowEvent::KeyReleased { text: slint::platform::Key::Control.into() });
}

fn save(window: &MinimalSoftwareWindow, path: &std::path::Path) {
    let (w, h) = (480usize, 150usize);
    let mut buffer = vec![Rgb8Pixel { r: 0, g: 0, b: 0 }; w * h];
    for _ in 0..3 {
        slint::platform::update_timers_and_animations();
        window.request_redraw();
        window.draw_if_needed(|renderer| {
            renderer.render(&mut buffer, w);
        });
    }
    let bytes: Vec<u8> = buffer.iter().flat_map(|p| [p.r, p.g, p.b]).collect();
    image::RgbImage::from_raw(w as u32, h as u32, bytes).expect("a full buffer").save(path).expect("the picture is written");
}

fn main() {
    let out = std::path::PathBuf::from(std::env::args().nth(1).expect("a picture folder"));
    std::fs::create_dir_all(&out).expect("the folder");
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    let copied = Rc::new(RefCell::new(String::new()));
    slint::platform::set_platform(Box::new(Headless { window: window.clone(), copied: copied.clone() })).expect("one platform");
    window.set_size(PhysicalSize::new(480, 150));
    let ui = Probe::new().expect("the window");
    ui.set_first(StyledText::from_markdown("Use **cargo test** and `cargo clippy` before pushing.").expect("markdown"));
    ui.set_first_plain("Use cargo test and cargo clippy before pushing.".into());
    ui.set_second(StyledText::from_markdown("See [the notes](https://example.test/notes) for the details.").expect("markdown"));
    ui.set_second_plain("See the notes for the details.".into());
    ui.show().expect("shown");
    save(&window, &out.join("0-before.png"));

    let mut failures = Vec::new();
    // A drag over the first line of the styled paragraph, then Ctrl+C.
    drag(&window, (12.0, 20.0), (230.0, 20.0));
    ctrl_c(&window);
    save(&window, &out.join("1-styled-drag.png"));
    let styled = copied.borrow().clone();
    println!("styled paragraph, copied: {styled:?}");
    if !(styled.contains("Use cargo") && styled.contains("test")) {
        failures.push("a drag in a styled paragraph did not copy its text");
    }

    // A drag in the paragraph with a link (starting on plain words), then Ctrl+C.
    copied.borrow_mut().clear();
    drag(&window, (12.0, 57.0), (190.0, 57.0));
    ctrl_c(&window);
    save(&window, &out.join("2-link-drag.png"));
    let linked = copied.borrow().clone();
    println!("paragraph with a link, copied: {linked:?}");
    if !linked.contains("See") {
        failures.push("a drag in a paragraph with a link did not copy its text");
    }

    // A click (no drag) on the link still follows it.
    click(&window, (70.0, 57.0));
    println!("link followed: {:?}", ui.get_last_link());
    if ui.get_last_link() != "https://example.test/notes" {
        failures.push("a click on the link did not follow it");
    }

    if failures.is_empty() {
        println!("OK");
    } else {
        for f in &failures {
            eprintln!("FAILED: {f}");
        }
        std::process::exit(1);
    }
}
