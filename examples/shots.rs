//! Draws screens of the desktop window with Slint's software renderer into PNG files (no display needed), to look at the design:
//!   cargo run --features desktop --example shots -- <output folder>
//! The window is filled with sample content only; nothing of the real app runs.
use std::rc::Rc;

use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, WindowAdapter};
use slint::{ComponentHandle, ModelRc, PhysicalSize, Rgb8Pixel, SharedString, VecModel};
use tenrec::view::i18n::{tr, Lang};

slint::include_modules!();

struct Headless(Rc<MinimalSoftwareWindow>);

impl Platform for Headless {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.0.clone())
    }
}

const W: u32 = 1280;
const H: u32 = 800;

fn words(ui: &AppWindow) {
    let t = ui.global::<T>();
    t.on_tr(|key| tr(Lang::Ko, &key).into());
    let w = |k: &str| SharedString::from(tr(Lang::Ko, k));
    t.set_app_sub(w("app_sub"));
    t.set_tab_chats(w("tab_chats"));
    t.set_tab_memory(w("tab_memory"));
    t.set_tab_skills(w("tab_skills"));
    t.set_tab_sched(w("tab_sched"));
    t.set_tab_files(w("tab_files"));
    t.set_sessions(w("sessions"));
    t.set_new_chat(w("new_chat"));
    t.set_placeholder(w("placeholder"));
    t.set_send(w("send"));
    t.set_stop(w("stop"));
    t.set_hint(w("hint"));
    t.set_copy(w("copy"));
    t.set_connected(w("connected"));
    t.set_empty(w("empty"));
}

fn row(kind: &str, text: &str) -> ChatRow {
    ChatRow { kind: kind.into(), text: text.into(), ..Default::default() }
}

fn save(window: &MinimalSoftwareWindow, path: &std::path::Path) {
    slint::platform::update_timers_and_animations();
    let mut buffer = vec![Rgb8Pixel { r: 0, g: 0, b: 0 }; (W * H) as usize];
    // Two passes: the first lays the window out, the second draws what the layout settled.
    for _ in 0..2 {
        window.draw_if_needed(|renderer| {
            renderer.render(&mut buffer, W as usize);
        });
        slint::platform::update_timers_and_animations();
        window.request_redraw();
    }
    window.draw_if_needed(|renderer| {
        renderer.render(&mut buffer, W as usize);
    });
    let bytes: Vec<u8> = buffer.iter().flat_map(|p| [p.r, p.g, p.b]).collect();
    image::RgbImage::from_raw(W, H, bytes).expect("a full buffer").save(path).expect("the picture is written");
    println!("{}", path.display());
}

fn main() {
    let out = std::path::PathBuf::from(std::env::args().nth(1).expect("an output folder"));
    std::fs::create_dir_all(&out).expect("the folder");
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    slint::platform::set_platform(Box::new(Headless(window.clone()))).expect("one platform");
    window.set_size(PhysicalSize::new(W, H));
    let ui = AppWindow::new().expect("the window");
    words(&ui);
    ui.show().expect("shown");

    ui.set_has_chat(true);
    ui.set_chat_title("README 정리".into());
    ui.set_model_name("Opus 5.5".into());
    ui.set_state("idle".into());
    ui.set_state_label("대기".into());
    let rows = vec![
        row("day", "오늘 · 09:41"),
        row("meta-user", "나"),
        row("user", "/cost"),
        row("meta-asst", "/cost"),
        row("note", "Total cost: $0.00 (이 대화는 아직 비용이 없습니다)"),
        row("meta-user", "나"),
        row("user", "프로젝트 설정을 점검해 줘"),
        row("note-warn", "한도의 80%에 가까워지고 있습니다."),
    ];
    ui.set_rows(ModelRc::from(Rc::new(VecModel::from(rows))));

    // 1. Dark, with the "/" menu.
    ui.global::<Theme>().set_mode(0);
    ui.set_draft("/co".into());
    let g = ui.global::<Slash>();
    let rows = vec![
        SlashRow { name: "compact".into(), about: tr(Lang::Ko, "slash_compact").into(), arg: true, confirm: false, builtin: true },
        SlashRow { name: "context".into(), about: tr(Lang::Ko, "slash_context").into(), arg: false, confirm: false, builtin: true },
        SlashRow { name: "cost".into(), about: tr(Lang::Ko, "slash_cost").into(), arg: false, confirm: false, builtin: true },
        SlashRow { name: "code-review".into(), about: "".into(), arg: true, confirm: false, builtin: false },
        SlashRow { name: "my-skill".into(), about: "".into(), arg: true, confirm: false, builtin: false },
    ];
    g.set_rows(ModelRc::from(Rc::new(VecModel::from(rows))));
    g.set_sel(1);
    g.set_note("".into());
    g.set_open(true);
    save(&window, &out.join("1-dark-slash-menu.png"));

    // 2. Dark, with the project settings card.
    g.set_open(false);
    ui.set_draft("".into());
    let p = ui.global::<Proj>();
    let lines = tenrec::view::project_check::lines(
        &serde_json::json!({"hash": "h", "findings": [
            {"severity": "danger", "file": ".claude/settings.json", "key": "hooks", "text": "Stop (./x.sh)"},
            {"severity": "danger", "file": ".claude/settings.json", "key": "env_redirect", "text": "ANTHROPIC_BASE_URL=•••, BASH_ENV=•••"},
            {"severity": "warn", "file": ".mcp.json", "key": "mcp_command", "text": "db (npx)"},
            {"severity": "info", "file": ".claude/settings.json", "key": "env", "text": "DISABLE_TELEMETRY=•••"},
        ]}),
        Lang::Ko,
    );
    let lines: Vec<WtLine> = lines.into_iter().map(|(text, bad)| WtLine { text: text.into(), bad }).collect();
    p.set_lines(ModelRc::from(Rc::new(VecModel::from(lines))));
    p.set_show(true);
    save(&window, &out.join("2-dark-project-card.png"));
    p.set_show(false);

    // 3. Light, with the menu again (the light palette and its contrast).
    ui.global::<Theme>().set_mode(1);
    ui.set_draft("/".into());
    g.set_open(true);
    save(&window, &out.join("3-light-slash-menu.png"));
    g.set_open(false);
    ui.set_draft("".into());
    p.set_show(true);
    save(&window, &out.join("4-light-project-card.png"));
}
