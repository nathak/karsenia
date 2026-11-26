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

fn save(window: &MinimalSoftwareWindow, path: &std::path::Path, w: u32, h: u32) {
    window.set_size(PhysicalSize::new(w, h));
    slint::platform::update_timers_and_animations();
    let mut buffer = vec![Rgb8Pixel { r: 0, g: 0, b: 0 }; (w * h) as usize];
    // Two passes: the first lays the window out, the second draws what the layout settled.
    for _ in 0..2 {
        window.draw_if_needed(|renderer| {
            renderer.render(&mut buffer, w as usize);
        });
        slint::platform::update_timers_and_animations();
        window.request_redraw();
    }
    window.draw_if_needed(|renderer| {
        renderer.render(&mut buffer, w as usize);
    });
    let bytes: Vec<u8> = buffer.iter().flat_map(|p| [p.r, p.g, p.b]).collect();
    image::RgbImage::from_raw(w, h, bytes).expect("a full buffer").save(path).expect("the picture is written");
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
    save(&window, &out.join("1-dark-slash-menu.png"), W, H);

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
    save(&window, &out.join("2-dark-project-card.png"), W, H);
    p.set_show(false);

    // 3. Light, with the menu again (the light palette and its contrast).
    ui.global::<Theme>().set_mode(1);
    ui.set_draft("/".into());
    g.set_open(true);
    save(&window, &out.join("3-light-slash-menu.png"), W, H);
    g.set_open(false);
    ui.set_draft("".into());
    p.set_show(true);
    save(&window, &out.join("4-light-project-card.png"), W, H);

    // 5. The window with a chat open, at different widths and with few or many accounts: the sidebar's account choice and the session header.
    scenes(&ui, &window, &out);
}

fn strings(list: &[&str]) -> ModelRc<SharedString> {
    ModelRc::from(Rc::new(VecModel::from(list.iter().map(|s| SharedString::from(*s)).collect::<Vec<_>>())))
}

/// A chat is open: a long title, the account, a permission mode, a worktree chip, a gateway and a model to choose.
fn open_chat(ui: &AppWindow, accounts: &[&str]) {
    ui.set_has_chat(true);
    ui.set_chat_title("릴리스 점검 및 사용량 카드 개선 작업 정리 (긴 제목 확인용)".into());
    ui.set_chat_account("회사-프로덕션-서울리전".into());
    ui.set_chat_perm("acceptEdits".into());
    ui.set_chat_perm_danger(false);
    ui.set_pwd_path("~/work/karsenia".into());
    ui.set_pwd_full("/home/me/work/karsenia".into());
    ui.set_crumb_ws("work".into());
    ui.set_crumb_proj("karsenia".into());
    ui.set_crumb_branch("main".into());
    ui.set_turn_tokens("45k / 200k · 22%".into());
    ui.set_turn_cost("$0.42".into());
    ui.set_model_idx(2);
    ui.set_gw_shown(true);
    ui.set_gw_names(strings(&["게이트웨이 없음", "Air"]));
    ui.set_gw_idx(0);
    ui.set_state("idle".into());
    ui.set_state_label("대기".into());
    ui.global::<Wt>().set_show(true);
    let acct = ui.global::<Acct>();
    acct.set_names(strings(accounts));
    acct.set_labels(strings(accounts));
    acct.set_view(0);
    ui.set_renaming(false);
    ui.set_text_view(false);
    let rows = vec![
        row("day", "오늘 · 09:41"),
        row("meta-user", "나"),
        row("user", "릴리스 전에 확인할 것을 정리해 줘"),
        row("meta-asst", "opus"),
        row("para", "확인할 것은 세 가지입니다."),
    ];
    ui.set_rows(ModelRc::from(Rc::new(VecModel::from(rows))));
}

fn scenes(ui: &AppWindow, window: &MinimalSoftwareWindow, out: &std::path::Path) {
    let few = ["기본 계정", "업무", "개인"];
    let many = [
        "기본 계정", "⚠ 2 회사-프로덕션-서울리전-계정", "● 1 개인", "연구소", "파트너사-A", "파트너사-B", "테스트-계정-1", "테스트-계정-2", "테스트-계정-3", "고객지원", "운영", "감사",
    ];
    // What the earlier pictures left open is shut: the pictures below are of the window itself.
    ui.global::<Proj>().set_show(false);
    ui.global::<Slash>().set_open(false);
    ui.set_draft("".into());
    ui.global::<Theme>().set_mode(0);
    ui.set_side(0);
    open_chat(ui, &few);
    save(window, &out.join("5-1280x800-3acct-dark.png"), 1280, 800);
    open_chat(ui, &many);
    // What waits elsewhere, beside the closed choice.
    ui.global::<Acct>().set_away("⚠ 2".into());
    save(window, &out.join("6-1280x800-12acct-long-dark.png"), 1280, 800);
    save(window, &out.join("7-1000x700-12acct-chat-dark.png"), 1000, 700);
    save(window, &out.join("8-760x640-12acct-chat-dark.png"), 760, 640);
    ui.set_renaming(true);
    ui.set_rename_text("릴리스 점검 및 사용량 카드 개선 작업 정리 (긴 제목 확인용)".into());
    save(window, &out.join("9-760x640-renaming-dark.png"), 760, 640);
    ui.set_renaming(false);
    ui.set_text_view(true);
    save(window, &out.join("10-760x640-textview-dark.png"), 760, 640);
    ui.set_text_view(false);
    // The "more" menu of the action row: at 1000px only debug and "continue in another account" are in it, at 760px everything that is
    // not always shown; "continue" opens its own card at the same place.
    ui.set_more_open(true);
    save(window, &out.join("13-1000x700-more-dark.png"), 1000, 700);
    save(window, &out.join("14-760x640-more-dark.png"), 760, 640);
    ui.set_more_open(false);
    ui.set_cont_open(true);
    save(window, &out.join("15-760x640-cont-dark.png"), 760, 640);
    ui.set_cont_open(false);
    // One account, a window wide enough for the gateway and copy but not for debug: the menu has debug only.
    open_chat(ui, &["기본 계정"]);
    save(window, &out.join("16-1160x800-1acct-dark.png"), 1160, 800);
    // The new-chat form with what was remembered for the folder filled in (the line that says so, and the way back).
    ui.set_new_open(true);
    ui.set_new_cwd("/home/me/work/karsenia".into());
    // Opening the form clears what was filled in (and then asks for the folder's); the picture is taken after that.
    save(window, &out.join("17a-newchat-open-dark.png"), 1280, 1000);
    ui.global::<NewForm>().set_model(1);
    ui.set_new_perm(2);
    ui.global::<NewForm>().set_used(true);
    ui.global::<NewForm>().set_recent(ModelRc::from(Rc::new(VecModel::from(vec![SharedString::from("/home/me/work/karsenia"), SharedString::from("/home/me/work/blog"), SharedString::from("/srv/clients/acme-portal")]))));
    save(window, &out.join("17-1280x800-newchat-defaults-dark.png"), 1280, 1000);
    ui.global::<NewForm>().set_used(false);
    // A trust remembered for the folder, and the way to forget it.
    ui.global::<NewForm>().set_trusted(true);
    save(window, &out.join("17b-newchat-trusted-dark.png"), 1280, 1000);
    ui.global::<NewForm>().set_trusted(false);
    ui.set_new_open(false);
    // The card that asks about the project's own settings, with the choice to remember the decision for the project.
    let proj = ui.global::<Proj>();
    proj.set_lines(ModelRc::from(Rc::new(VecModel::from(vec![WtLine { text: "위험 · .claude/settings.json · 훅이 명령을 실행합니다: Stop → ./x.sh".into(), bad: true }]))));
    proj.set_can_remember(true);
    proj.set_show(true);
    save(window, &out.join("18-1280x800-project-card-dark.png"), 1280, 800);
    proj.set_show(false);
    open_chat(ui, &many);
    ui.global::<Theme>().set_mode(1);
    save(window, &out.join("11-1280x800-12acct-light.png"), 1280, 800);
    save(window, &out.join("12-760x640-12acct-chat-light.png"), 760, 640);
}
