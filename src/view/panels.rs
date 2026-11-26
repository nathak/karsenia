//! The Memory and Skills panels and the search results, turned into flat rows for the desktop app. The data is what the web
//! page receives (the `memory`, `skills` and `search_results` replies); nothing here talks to a store.
use std::collections::{HashMap, HashSet};

use serde_json::Value;

use super::i18n::{fill, tr, Lang};
use super::markdown::escape;
use crate::index::{MARK_CLOSE, MARK_OPEN};

/// How long ago, short: "방금" / "now", then minutes, hours, days.
pub fn ago(ms: u64, now: u64, lang: Lang) -> String {
    let s = now.saturating_sub(ms) / 1000;
    match s {
        0..=59 => if lang == Lang::Ko { "방금".into() } else { "now".into() },
        60..=3599 => format!("{}m", s / 60),
        3600..=86399 => format!("{}h", s / 3600),
        _ => format!("{}d", s / 86400),
    }
}

/// The last part of a folder path, for a short label.
pub fn folder_name(path: &str) -> String {
    path.trim_end_matches(['/', '\\']).rsplit(['/', '\\']).next().unwrap_or(path).to_owned()
}

fn s(v: &Value, key: &str) -> String {
    v[key].as_str().unwrap_or_default().to_owned()
}

fn id(v: &Value) -> String {
    v["id"].as_i64().map(|n| n.to_string()).unwrap_or_default()
}

// ---- memory ----

#[derive(Debug, Clone, Default, PartialEq)]
pub struct MemHead {
    pub available: bool,
    pub notice: String,
    /// 0 off, 1 this project, 2 project and the person.
    pub auto: i32,
    pub has_cwd: bool,
    /// The first-run card about auto-save has not been answered.
    pub card: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct MemRow {
    /// head, note or empty.
    pub kind: String,
    /// The section's key for a head; "<section>:<id>" for a note.
    pub key: String,
    /// pending, project, user, recent, others or deleted: what a note can do.
    pub section: String,
    pub id: String,
    pub text: String,
    pub meta: String,
    pub pinned: bool,
    pub instr: bool,
    pub open: bool,
    pub count: i32,
}

/// Sections closed until opened.
pub const MEM_CLOSED: [&str; 3] = ["recent", "others", "deleted"];

/// Whether a section is open: the closed ones are open when listed in `toggled`, the others when not.
pub fn is_open(key: &str, closed_by_default: &[&str], toggled: &HashSet<String>) -> bool {
    closed_by_default.contains(&key) == toggled.contains(key)
}

/// What the search box and the "suggested" chip leave of a list: a head stays only while a row under it does, an empty-section line goes
/// while a search is on. `text` and `pending` say what a row is for the filter.
pub fn keep_matching<T: Clone>(rows: &[T], filter: &str, only_pending: bool, kind: impl Fn(&T) -> &str, text: impl Fn(&T) -> String, pending: impl Fn(&T) -> bool) -> Vec<T> {
    let needle = filter.trim().to_lowercase();
    if needle.is_empty() && !only_pending {
        return rows.to_vec();
    }
    let mut out: Vec<T> = Vec::new();
    let mut head: Option<T> = None;
    for r in rows {
        match kind(r) {
            "head" => head = Some(r.clone()),
            "empty" => {}
            _ => {
                if (needle.is_empty() || text(r).to_lowercase().contains(&needle)) && (!only_pending || pending(r)) {
                    if let Some(h) = head.take() {
                        out.push(h);
                    }
                    out.push(r.clone());
                }
            }
        }
    }
    out
}

pub fn memory_view(data: &Value, lang: Lang, now: u64, toggled: &HashSet<String>) -> (MemHead, Vec<MemRow>) {
    let settings = &data["settings"];
    let head = MemHead {
        available: data["available"] == true,
        notice: s(data, "notice"),
        auto: match settings["memory_auto"].as_str() {
            Some("project") => 1,
            Some("project_user") => 2,
            _ => 0,
        },
        has_cwd: data["cwd"].as_str().is_some_and(|c| !c.is_empty()),
        card: data["available"] == true && settings["memory_card_seen"] != true,
    };
    let mut rows = Vec::new();
    if !head.available {
        return (head, rows);
    }
    let overview = &data["overview"];
    let project_title = match data["cwd"].as_str().filter(|c| !c.is_empty()) {
        Some(cwd) => format!("{} · {}", tr(lang, "mem_project"), folder_name(cwd)),
        None => tr(lang, "mem_project"),
    };
    let sections = [
        ("pending", "pending", tr(lang, "mem_pending")),
        ("project", "project", project_title),
        ("user", "user", tr(lang, "mem_user")),
        ("recent", "recent_auto", tr(lang, "mem_recent")),
        ("others", "others", tr(lang, "mem_others")),
        ("deleted", "deleted", tr(lang, "mem_deleted")),
    ];
    let mut any = false;
    for (key, field, title) in sections {
        let notes = overview[field].as_array().cloned().unwrap_or_default();
        // The pending, project and person sections are shown even when empty only if something else is there: an empty
        // panel shows one line instead.
        if notes.is_empty() {
            continue;
        }
        any = true;
        let open = is_open(key, &MEM_CLOSED, toggled);
        rows.push(MemRow { kind: "head".into(), key: key.into(), section: key.into(), text: title, open, count: notes.len() as i32, ..Default::default() });
        if !open {
            continue;
        }
        for note in &notes {
            let source = match note["source"].as_str() {
                Some("claude") => tr(lang, "mem_src_claude"),
                Some("auto") => tr(lang, "mem_src_auto"),
                _ => tr(lang, "mem_src_manual"),
            };
            let when = note["deleted_at"].as_u64().filter(|_| key == "deleted").or(note["updated"].as_u64()).unwrap_or(0);
            let mut meta = format!("{source} · {}", ago(when, now, lang));
            if key == "others" {
                if let Some(cwd) = note["cwd"].as_str() {
                    meta = format!("{} · {meta}", folder_name(cwd));
                }
            }
            rows.push(MemRow {
                kind: "note".into(),
                key: format!("{key}:{}", id(note)),
                section: key.into(),
                id: id(note),
                text: s(note, "text"),
                meta,
                pinned: note["pinned"] == true,
                instr: note["instr"] == true,
                ..Default::default()
            });
        }
    }
    if !any {
        rows.push(MemRow { kind: "empty".into(), key: "empty".into(), text: tr(lang, "mem_empty"), ..Default::default() });
    }
    (head, rows)
}

// ---- skills ----

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SkHead {
    pub available: bool,
    pub notice: String,
    pub auto: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SkillRow {
    /// head, proposal, skill or empty.
    pub kind: String,
    pub key: String,
    /// proposals, active or deleted.
    pub section: String,
    pub id: String,
    pub name: String,
    pub meta: String,
    pub description: String,
    /// SKILL.md as it would be saved (a proposal) or the current body (a skill).
    pub body: String,
    /// One line per finding: "line 3 · code · excerpt".
    pub risks: String,
    pub has_block: bool,
    pub has_warn: bool,
    pub enabled: bool,
    /// A proposal to improve a skill (its name stays).
    pub improve: bool,
    /// The active version of a skill.
    pub version: i32,
    pub open: bool,
    pub count: i32,
}

/// One line per finding: "must fix · line 3 · code · excerpt".
pub fn risk_lines(risks: &[Value], lang: Lang) -> String {
    risks
        .iter()
        .map(|r| {
            let level = if r["level"] == "block" { tr(lang, "sk_blocked") } else { tr(lang, "sk_warn") };
            let place = match r["line"].as_u64() {
                Some(0) | None => String::new(),
                Some(n) => format!("{} · ", fill(&tr(lang, "sk_line"), &[("n", n.to_string())])),
            };
            format!("{level} · {place}{} · {}", s(r, "code"), s(r, "excerpt"))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SkVer {
    pub n: i32,
    pub meta: String,
    pub description: String,
    pub current: bool,
}

/// The versions of a skill, newest first as the server lists them; `current` is the active one.
pub fn versions_view(data: &Value, current: i64, lang: Lang, now: u64) -> Vec<SkVer> {
    data["versions"]
        .as_array()
        .map(|list| {
            list.iter()
                .map(|v| {
                    let n = v["n"].as_i64().unwrap_or(0);
                    let source = match v["source"].as_str() {
                        Some("claude") => tr(lang, "mem_src_claude"),
                        Some("auto") => tr(lang, "mem_src_auto"),
                        _ => tr(lang, "mem_src_manual"),
                    };
                    SkVer { n: n as i32, meta: format!("v{n} · {source} · {}", ago(v["created"].as_u64().unwrap_or(0), now, lang)), description: s(v, "description"), current: n == current }
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Skill sections closed until opened.
pub const SK_CLOSED: [&str; 1] = ["deleted"];

pub fn skills_view(data: &Value, lang: Lang, now: u64, toggled: &HashSet<String>) -> (SkHead, Vec<SkillRow>) {
    let head = SkHead { available: data["available"] == true, notice: s(data, "notice"), auto: data["auto"] == true };
    let mut rows = Vec::new();
    if !head.available {
        return (head, rows);
    }
    let overview = &data["overview"];
    let scope = |v: &Value| if v["scope"] == "user" { tr(lang, "sk_scope_user") } else { v["cwd"].as_str().map(folder_name).unwrap_or_else(|| tr(lang, "sk_scope_project")) };
    let mut any = false;
    for (key, title) in [("proposals", tr(lang, "sk_pending")), ("active", tr(lang, "sk_active")), ("deleted", tr(lang, "sk_deleted"))] {
        let items = overview[key].as_array().cloned().unwrap_or_default();
        if items.is_empty() {
            continue;
        }
        any = true;
        let open = is_open(key, &SK_CLOSED, toggled);
        rows.push(SkillRow { kind: "head".into(), key: key.into(), section: key.into(), name: title, open, count: items.len() as i32, ..Default::default() });
        if !open {
            continue;
        }
        for item in &items {
            if key == "proposals" {
                let risks = item["risks"].as_array().cloned().unwrap_or_default();
                let lines = risk_lines(&risks, lang);
                let target = item["target"].as_str().filter(|t| !t.is_empty());
                let mut meta = scope(item);
                if target.is_some() {
                    meta = format!("{} · {meta}", tr(lang, "sk_improve_tag"));
                }
                if item["from_gateway"] == true {
                    meta = format!("{} · {meta}", tr(lang, "sk_gateway_tag"));
                }
                meta = format!("{meta} · {}", ago(item["created"].as_u64().unwrap_or(0), now, lang));
                rows.push(SkillRow {
                    kind: "proposal".into(),
                    key: format!("p:{}", id(item)),
                    section: key.into(),
                    id: id(item),
                    name: s(item, "name"),
                    meta,
                    description: s(item, "description"),
                    body: s(item, "body"),
                    risks: lines,
                    improve: target.is_some(),
                    has_block: risks.iter().any(|r| r["level"] == "block"),
                    has_warn: risks.iter().any(|r| r["level"] == "warn"),
                    ..Default::default()
                });
            } else {
                let enabled = item["status"] == "active";
                let used = match item["last_used"].as_u64() {
                    Some(t) => format!("{} {}", tr(lang, "sk_last_used"), ago(t, now, lang)),
                    None => tr(lang, "sk_never"),
                };
                let mut meta = format!("{} · v{} · {used}", scope(item), item["version"].as_i64().unwrap_or(1));
                if key == "active" && !enabled {
                    meta = format!("{} · {meta}", tr(lang, "sk_disabled_tag"));
                }
                rows.push(SkillRow {
                    kind: "skill".into(),
                    key: format!("{key}:{}", id(item)),
                    section: key.into(),
                    id: id(item),
                    name: s(item, "name"),
                    meta,
                    description: s(item, "description"),
                    body: s(item, "body"),
                    enabled,
                    version: item["version"].as_i64().unwrap_or(1) as i32,
                    ..Default::default()
                });
            }
        }
    }
    if !any {
        rows.push(SkillRow { kind: "empty".into(), key: "empty".into(), name: tr(lang, "sk_empty"), ..Default::default() });
    }
    (head, rows)
}

// ---- search ----

#[derive(Debug, Clone, Default, PartialEq)]
pub struct HitRow {
    pub sid: String,
    pub title: String,
    pub meta: String,
    /// The snippet as safe markdown, the match in bold.
    pub md: String,
    pub plain: String,
}

/// A snippet with MARK_OPEN / MARK_CLOSE around the matches: escaped markdown with the matches in bold, and the plain text.
/// Line breaks become spaces; a mark without its pair is dropped.
pub fn snippet_md(snippet: &str) -> (String, String) {
    let flat: String = snippet.chars().map(|c| if c == '\n' || c == '\r' || c == '\t' { ' ' } else { c }).collect();
    let (mut md, mut plain, mut part) = (String::new(), String::new(), String::new());
    let mut inside = false;
    let close = |md: &mut String, part: &mut String, bold: bool| {
        let text = std::mem::take(part);
        let core = text.trim();
        if bold && !core.is_empty() {
            // Spaces stay outside the marks: `** x**` is not bold in markdown.
            let (lead, tail) = (&text[..text.len() - text.trim_start().len()], &text[text.trim_end().len()..]);
            md.push_str(&format!("{lead}**{}**{tail}", escape(core)));
        } else {
            md.push_str(&escape(&text));
        }
    };
    for c in flat.chars() {
        match c {
            MARK_OPEN if !inside => {
                close(&mut md, &mut part, false);
                inside = true;
            }
            MARK_CLOSE if inside => {
                close(&mut md, &mut part, true);
                inside = false;
            }
            MARK_OPEN | MARK_CLOSE => {}
            c if c.is_control() => {}
            c => {
                part.push(c);
                plain.push(c);
            }
        }
    }
    close(&mut md, &mut part, inside);
    // Leading spaces would make an indented code block.
    (md.trim_start().to_owned(), plain)
}

pub fn hits_view(data: &Value, lang: Lang, now: u64) -> Result<Vec<HitRow>, String> {
    if let Some(error) = data["error"].as_str() {
        return Err(error.to_owned());
    }
    Ok(data["hits"]
        .as_array()
        .map(|hits| {
            hits.iter()
                .map(|hit| {
                    let (md, plain) = snippet_md(hit["snippet"].as_str().unwrap_or_default());
                    let title = s(hit, "title");
                    let title = if title.is_empty() { tr(lang, "untitled") } else { title };
                    let role = if hit["role"] == "user" { tr(lang, "you") } else { "Claude".to_owned() };
                    HitRow { sid: s(hit, "sid"), title, meta: format!("{role} · {} · {}", folder_name(&s(hit, "cwd")), ago(hit["ts"].as_u64().unwrap_or(0), now, lang)), md, plain }
                })
                .collect()
        })
        .unwrap_or_default())
}

// ---- accounts ----

#[derive(Debug, Clone, Default, PartialEq)]
pub struct AcctHead {
    pub notice: String,
    /// Fewer than the accounts the app allows (`accounts::MAX_ACCOUNTS`, both kinds together).
    pub can_add: bool,
    /// The account picker's entries, the default account first, and the id behind each (None for the default one). Only Claude accounts:
    /// a chat runs in one of them, and a Codex account has no chats, tabs or notes to pick.
    pub names: Vec<String>,
    pub ids: Vec<Option<String>>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct AcctRow {
    /// "" for the default account.
    pub id: String,
    pub name: String,
    pub sub: String,
    /// never, busy, in, out, error or closed.
    pub state: String,
    pub chats: usize,
    pub open: bool,
    pub is_default: bool,
    /// "claude" or "codex" (the default account is a Claude one).
    pub kind: String,
    /// What logs this account in from a terminal.
    pub login: String,
    /// The folder this account's `claude` keeps its login and history in.
    pub dir: String,
}

/// The Accounts card: the default account, then each extra one. `status` holds what `account_status` said per account
/// ("" for the default one): no key when never asked, None while asking. `chats` counts the chats of each account.
pub fn accounts_view(list: &Value, status: &HashMap<String, Option<Value>>, chats: &HashMap<String, usize>, lang: Lang) -> (AcctHead, Vec<AcctRow>) {
    let accounts = list["accounts"].as_array().cloned().unwrap_or_default();
    let mut head = AcctHead { notice: s(list, "notice"), can_add: accounts.len() < crate::accounts::MAX_ACCOUNTS, names: vec![tr(lang, "acct_default")], ids: vec![None] };
    // The state goes on the row's pill; the line under it says only what the pill does not (the email, the error).
    let describe = |key: &str, open: bool| -> (String, String) {
        if !open {
            return ("closed".into(), String::new());
        }
        match status.get(key) {
            None => ("never".into(), String::new()),
            Some(None) => ("busy".into(), String::new()),
            Some(Some(data)) => match data.get("error").and_then(Value::as_str) {
                Some(e) => ("error".into(), fill(&tr(lang, "acct_st_failed"), &[("e", e.to_owned())])),
                // A Codex account's answer has its own shape (see `codex::CodexAuth`): how it was logged in, and whether the login is in its folder.
                None if data["kind"] == "codex" => {
                    let st = &data["status"];
                    if st["known"] == false {
                        ("error".into(), tr(lang, "acct_codex_unknown"))
                    } else if st["logged_in"] == true {
                        let how = match st["method"].as_str() {
                            Some("chatgpt") => tr(lang, "acct_codex_chatgpt"),
                            Some("api_key") => tr(lang, "acct_codex_api_key"),
                            _ => tr(lang, "acct_codex_other"),
                        };
                        ("in".into(), if st["file"] == false { format!("{how} · {}", tr(lang, "acct_codex_file_missing")) } else { how })
                    } else {
                        ("out".into(), String::new())
                    }
                }
                None if data["status"]["logged_in"] == true => {
                    let st = &data["status"];
                    let mut parts = Vec::new();
                    for key in ["email", "org", "subscription"] {
                        if let Some(v) = st[key].as_str().filter(|v| !v.is_empty()) {
                            parts.push(v.to_owned());
                        }
                    }
                    ("in".into(), parts.join(" · "))
                }
                None => ("out".into(), String::new()),
            },
        }
    };
    let row = |id: &str, name: String, kind: &str, open: bool, login: String, dir: String| {
        let (state, mut sub) = describe(id, open);
        let n = chats.get(id).copied().unwrap_or(0);
        if n > 0 {
            let count = fill(&tr(lang, "acct_chats"), &[("n", n.to_string())]);
            sub = if sub.is_empty() { count } else { format!("{count} · {sub}") };
        }
        AcctRow { id: id.to_owned(), name, sub, state, chats: n, open, is_default: id.is_empty(), kind: kind.to_owned(), login, dir }
    };
    // The default account lives where `claude` keeps its own files (CLAUDE_CONFIG_DIR, else ~/.claude).
    let default_dir = crate::accounts::default_config_dir(std::env::var_os("CLAUDE_CONFIG_DIR").map(std::path::PathBuf::from).as_deref(), crate::platform::home_dir().as_deref()).map(|d| d.display().to_string()).unwrap_or_default();
    let mut rows = vec![row("", tr(lang, "acct_default"), "claude", true, "claude auth login".to_owned(), default_dir)];
    // The Claude accounts, then the Codex ones (each in the order the host listed them).
    for kind in ["claude", "codex"] {
        for a in accounts.iter().filter(|a| a["kind"].as_str().unwrap_or("claude") == kind) {
            let id = s(a, "id");
            let name = s(a, "name");
            if kind == "claude" {
                head.names.push(name.clone());
                head.ids.push(Some(id.clone()));
            }
            rows.push(row(&id, name, kind, a["open"] != false, s(a, "login"), s(a, "dir")));
        }
    }
    (head, rows)
}

/// What the chat list is of: an account (None is the default one). A chat belongs to its account whatever provider it runs on: the
/// providers are not tabs or entries of the account choice.
pub type View = Option<String>;

/// The ids the host listed, to tell a chat of something that was deleted from one that is only not listed yet. `None` is "the list has not
/// come": a chat of an extra account is then taken to be listed under its own.
#[derive(Clone, Copy, Default)]
pub struct Known<'a> {
    pub accounts: Option<&'a [String]>,
}

/// The account a chat is shown under: its own, or None (the default one) when it has none or its account is gone.
pub fn view_of(chat: Option<&str>, known: Option<&[String]>) -> Option<String> {
    chat.filter(|a| known.is_none_or(|k| k.iter().any(|id| id == a))).map(str::to_owned)
}

/// One chat as the account choice and the list see it: its account id, whether it is working, how many questions wait for the person.
#[derive(Clone)]
pub struct Mark {
    pub account: Option<String>,
    pub running: bool,
    pub pending: usize,
    /// Put away by the person (see `listed`).
    pub archived: bool,
}

/// Whether a chat is in the list: not archived, or the archive is shown, or it needs the person's eyes anyway (it works, or a question
/// waits), or it is the open one (it goes when another chat is opened). What the account choice counts (`attention`) is not affected.
pub fn listed(chat: &Mark, show_archived: bool, open: bool) -> bool {
    !chat.archived || show_archived || chat.running || chat.pending > 0 || open
}

/// The account a chat is listed under: its own, or the default one when it has none or its account was deleted, so no chat becomes
/// unreachable. A chat on a provider with its own key and one on a gateway that uses the account's login are listed the same way.
pub fn owner(chat: &Mark, known: Known) -> View {
    view_of(chat.account.as_deref(), known.accounts)
}

/// True when a chat is listed under the account being shown.
pub fn in_context(chat: &Mark, view: &View, known: Known) -> bool {
    owner(chat, known) == *view
}

/// What waits or runs under the accounts that are not shown: for each account the number of chats with a question waiting and the number
/// that are working. A chat that waits is not counted as working too.
pub fn attention(chats: &[Mark], view: &View, known: Known) -> HashMap<View, (usize, usize)> {
    let mut out: HashMap<View, (usize, usize)> = HashMap::new();
    for c in chats {
        let owner = owner(c, known);
        if owner == *view {
            continue;
        }
        let e = out.entry(owner).or_default();
        if c.pending > 0 {
            e.0 += 1;
        } else if c.running {
            e.1 += 1;
        }
    }
    out
}

/// The entries of the account choice, in order (`ids` and `names` as the host listed them, the default account first), each with what waits
/// or runs under it when it is not the account shown.
pub fn labels(ids: &[View], names: &[String], seen: &HashMap<View, (usize, usize)>) -> Vec<String> {
    ids.iter().zip(names).map(|(id, name)| tab_label(name, seen.get(id))).collect()
}

/// What waits or runs under all the accounts that are not shown, for the badge beside the closed choice (which names only the account
/// shown, so a question waiting elsewhere would otherwise go unseen): "⚠ n" when chats wait for the person, else "● n" when some are working,
/// else "".
pub fn away(seen: &HashMap<View, (usize, usize)>) -> String {
    let (wait, run) = seen.values().fold((0, 0), |(w, r), (sw, sr)| (w + sw, r + sr));
    if wait > 0 {
        format!("⚠ {wait}")
    } else if run > 0 {
        format!("● {run}")
    } else {
        String::new()
    }
}

/// An account's name in the header with what waits or runs under it: "⚠ 2 Work" (waiting), "● Work" (working).
pub fn tab_label(name: &str, seen: Option<&(usize, usize)>) -> String {
    match seen {
        Some((wait, _)) if *wait > 0 => format!("⚠ {wait} {name}"),
        Some((_, run)) if *run > 0 => format!("● {name}"),
        _ => name.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_search_and_the_suggested_chip_leave_only_matching_rows_under_their_heads() {
        let row = |kind: &str, section: &str, text: &str| MemRow { kind: kind.into(), section: section.into(), text: text.into(), ..Default::default() };
        let rows = vec![row("head", "", "Waiting"), row("note", "pending", "Use TLS 1.3"), row("head", "", "This project"), row("note", "project", "pnpm workspaces"), row("note", "project", "tls only"), row("empty", "", "none")];
        let keep = |f: &str, p: bool| keep_matching(&rows, f, p, |r| r.kind.as_str(), |r| r.text.clone(), |r| r.section == "pending").iter().map(|r| r.text.clone()).collect::<Vec<_>>();
        assert_eq!(keep("", false).len(), 6, "no filter leaves everything");
        assert_eq!(keep("TLS", false), vec!["Waiting", "Use TLS 1.3", "This project", "tls only"]);
        assert_eq!(keep("", true), vec!["Waiting", "Use TLS 1.3"]);
        assert_eq!(keep("pnpm", true), Vec::<String>::new());
        assert_eq!(keep("zzz", false), Vec::<String>::new());
    }

    use super::*;
    use serde_json::json;

    fn note(id: i64, text: &str, source: &str) -> Value {
        json!({"id": id, "scope": "project", "cwd": "/w/proj", "kind": "fact", "text": text, "source": source, "status": "active", "instr": false, "pinned": false, "created": 0, "updated": 1_000, "deleted_at": null})
    }

    #[test]
    fn memory_sections_in_order_closed_ones_hidden_until_opened() {
        let data = json!({"available": true, "cwd": "/w/proj", "settings": {"memory_auto": "project", "memory_card_seen": true},
            "overview": {"pending": [note(1, "p", "auto")], "project": [note(2, "a", "manual")], "user": [], "others": [note(3, "o", "claude")], "recent_auto": [], "deleted": []}});
        let (head, rows) = memory_view(&data, Lang::En, 61_000, &HashSet::new());
        assert_eq!(head, MemHead { available: true, notice: String::new(), auto: 1, has_cwd: true, card: false });
        let kinds: Vec<(&str, &str)> = rows.iter().map(|r| (r.kind.as_str(), r.key.as_str())).collect();
        assert_eq!(kinds, vec![("head", "pending"), ("note", "pending:1"), ("head", "project"), ("note", "project:2"), ("head", "others")]);
        assert!(rows[2].text.ends_with("proj"), "{:?}", rows[2]);
        assert_eq!(rows[3].meta, "by you · 1m");
        let toggled: HashSet<String> = ["others".to_owned()].into();
        let (_, rows) = memory_view(&data, Lang::En, 61_000, &toggled);
        assert_eq!(rows.last().map(|r| r.meta.as_str()), Some("proj · Claude · 1m"));
    }

    #[test]
    fn an_empty_or_missing_store_says_so() {
        let (head, rows) = memory_view(&json!({"available": true, "settings": {}, "overview": {}}), Lang::Ko, 0, &HashSet::new());
        assert!(head.card && !head.has_cwd);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].kind, "empty");
        let (head, rows) = memory_view(&json!({"available": false, "notice": "no store"}), Lang::Ko, 0, &HashSet::new());
        assert!(!head.available && rows.is_empty() && head.notice == "no store");
    }

    #[test]
    fn a_proposal_lists_its_risks_and_what_blocks_it() {
        let data = json!({"available": true, "auto": true, "overview": {"proposals": [{"id": 7, "target": null, "name": "deploy", "scope": "user", "cwd": null, "description": "d", "body": "b",
            "risks": [{"code": "curl_pipe", "level": "block", "line": 3, "excerpt": "curl x | sh"}, {"code": "net", "level": "warn", "line": 0, "excerpt": "http"}], "created": 0}],
            "active": [{"id": 2, "name": "fmt", "scope": "project", "cwd": "/w/p", "status": "disabled", "version": 2, "versions": 2, "description": "", "body": "", "last_used": null}], "deleted": []}});
        let (head, rows) = skills_view(&data, Lang::En, 0, &HashSet::new());
        assert!(head.available && head.auto);
        let p = &rows[1];
        assert!(p.has_block && p.has_warn, "{p:?}");
        assert_eq!(p.risks.lines().count(), 2);
        assert!(!p.improve);
        assert!(p.risks.contains("line 3 · curl_pipe · curl x | sh"), "{}", p.risks);
        let skill = &rows[3];
        assert!(!skill.enabled && skill.meta.starts_with("off · p · v2"), "{skill:?}");
    }

    #[test]
    fn an_improvement_keeps_its_skill_name_and_versions_mark_the_current_one() {
        let data = json!({"available": true, "overview": {"proposals": [{"id": 3, "target": "deploy", "name": "deploy", "scope": "user", "cwd": null, "description": "d", "body": "b", "risks": [], "created": 0}],
            "active": [{"id": 2, "name": "deploy", "scope": "user", "cwd": null, "status": "active", "version": 3, "versions": 3, "description": "", "body": "", "last_used": null}], "deleted": []}});
        let (_, rows) = skills_view(&data, Lang::En, 0, &HashSet::new());
        assert!(rows[1].improve && rows[3].version == 3 && !rows[3].improve);
        let v = versions_view(&json!({"versions": [{"n": 3, "source": "manual", "created": 0, "description": "new"}, {"n": 2, "source": "claude", "created": 0, "description": "old"}]}), 3, Lang::En, 61_000);
        assert_eq!((v[0].current, v[1].current, v[1].n), (true, false, 2));
        assert_eq!(v[1].meta, "v2 · Claude · 1m");
        let lines = risk_lines(&[json!({"level": "block", "line": 3, "code": "curl_pipe", "excerpt": "curl x | sh"}), json!({"level": "warn", "line": 0, "code": "net", "excerpt": "http"})], Lang::En);
        assert_eq!(lines.lines().count(), 2);
        assert!(lines.starts_with("must fix · line 3 · curl_pipe"), "{lines}");
    }

    #[test]
    fn a_gateway_proposal_row_carries_its_tag() {
        let data = json!({"available": true, "overview": {"proposals": [
            {"id": 1, "target": null, "name": "from-air", "scope": "user", "cwd": null, "description": "d", "body": "b", "risks": [{"level": "warn", "line": 0, "code": "from_gateway", "excerpt": "made from a chat that went through a gateway"}], "created": 0, "from_gateway": true},
            {"id": 2, "target": null, "name": "plain", "scope": "user", "cwd": null, "description": "d", "body": "b", "risks": [], "created": 0}], "active": [], "deleted": []}});
        for (lang, tag) in [(Lang::En, "From a gateway chat"), (Lang::Ko, "게이트웨이 대화에서 나옴")] {
            let (_, rows) = skills_view(&data, lang, 0, &HashSet::new());
            let (gateway, plain) = (&rows[1], &rows[2]);
            assert!(gateway.meta.starts_with(tag), "{}", gateway.meta);
            assert!(gateway.risks.contains("from_gateway"), "the warning is listed: {}", gateway.risks);
            assert!(!plain.meta.contains(tag), "{}", plain.meta);
        }
    }

    #[test]
    fn a_snippet_is_escaped_and_the_match_is_bold() {
        assert_eq!(snippet_md("    x \u{2} y \u{3} z").0, "x  **y**  z");
        let (md, plain) = snippet_md("a *b* \u{2}<x>\u{3} c\nd \u{2}open");
        assert_eq!(plain, "a *b* <x> c d open");
        assert!(md.contains(r"\*b\*"), "{md}");
        assert!(md.contains(r"**\<x\>**"), "{md}");
        assert!(md.ends_with("**open**"), "{md}");
        assert!(!md.contains('\u{2}') && !md.contains('\n'));
    }

    #[test]
    fn hits_carry_a_title_and_an_error_is_an_error() {
        let rows = hits_view(&json!({"hits": [{"sid": "s1", "seq": 1, "part": 0, "role": "user", "ts": 0, "cwd": "/w/proj", "snippet": "x", "title": ""}]}), Lang::En, 0).expect("hits");
        assert_eq!(rows[0].title, "new chat");
        assert_eq!(rows[0].meta, "you · proj · now");
        assert!(hits_view(&json!({"hits": [], "error": "no"}), Lang::En, 0).is_err());
    }

    #[test]
    fn accounts_view_lists_the_default_account_first_and_says_how_each_stands() {
        let list = json!({"notice": "n", "accounts": [{"id": "a1", "name": "Work", "open": true, "login": "L1"}, {"id": "a2", "name": "Old", "open": false, "login": "L2"}]});
        let mut status: HashMap<String, Option<Value>> = HashMap::new();
        status.insert("".into(), Some(json!({"status": {"logged_in": true, "email": "me@x", "org": "", "subscription": "max"}})));
        status.insert("a1".into(), None);
        let chats: HashMap<String, usize> = [("a1".to_owned(), 2)].into();
        let (head, rows) = accounts_view(&list, &status, &chats, Lang::En);
        assert_eq!(head.names, vec!["Default account", "Work", "Old"]);
        assert_eq!(head.ids, vec![None, Some("a1".into()), Some("a2".into())]);
        assert!(head.can_add && head.notice == "n");
        assert_eq!((rows[0].state.as_str(), rows[0].sub.as_str()), ("in", "me@x · max"));
        assert!(rows[0].is_default && rows[0].login == "claude auth login");
        assert_eq!((rows[1].state.as_str(), rows[1].sub.as_str()), ("busy", "2 chats"));
        assert_eq!(rows[2].state, "closed");
        status.insert("a1".into(), Some(json!({"error": "timeout"})));
        status.remove("");
        let (_, rows) = accounts_view(&list, &status, &HashMap::new(), Lang::Ko);
        assert_eq!(rows[0].state, "never");
        assert_eq!(rows[1].state, "error");
        assert!(rows[1].sub.contains("timeout"));
        status.insert("a1".into(), Some(json!({"status": {"logged_in": false}})));
        assert_eq!(accounts_view(&list, &status, &HashMap::new(), Lang::En).1[1].state, "out");
        let full = json!({"accounts": (0..8).map(|i| json!({"id": format!("a{i}"), "name": "x", "open": true, "login": ""})).collect::<Vec<_>>()});
        assert!(!accounts_view(&full, &HashMap::new(), &HashMap::new(), Lang::En).0.can_add);
    }
    fn mark(account: Option<&str>, running: bool, pending: usize) -> Mark {
        Mark { account: account.map(str::to_owned), running, pending, archived: false }
    }

    fn acct(id: Option<&str>) -> View {
        id.map(str::to_owned)
    }

    #[test]
    fn a_chat_is_listed_under_its_own_account_and_the_others_fall_to_the_default() {
        let accounts = vec!["a1".to_owned()];
        let k = Known { accounts: Some(&accounts) };
        let on = |account: Option<&str>, view: Option<&str>, k: Known| in_context(&mark(account, false, 0), &acct(view), k);
        assert!(on(None, None, k) && !on(None, Some("a1"), k));
        assert!(on(Some("a1"), Some("a1"), k) && !on(Some("a1"), None, k));
        // An account that was deleted: its chat is under the default account (and nowhere else).
        assert!(on(Some("zz"), None, k) && !on(Some("zz"), Some("a1"), k));
        // Before the list came, a chat of an extra account is under its own account only.
        let none = Known::default();
        assert!(on(Some("a1"), Some("a1"), none) && !on(Some("a1"), None, none) && !on(Some("zz"), None, none));
        assert_eq!(view_of(Some("a1"), k.accounts).as_deref(), Some("a1"));
        assert_eq!(view_of(Some("zz"), k.accounts), None);
        assert_eq!(view_of(Some("zz"), None).as_deref(), Some("zz"));
        assert_eq!(view_of(None, k.accounts), None);
    }

    #[test]
    fn a_provider_chat_is_listed_under_its_account_never_lost_and_never_a_tab_of_its_own() {
        // A chat on a provider with its own key has no account: it is under the default account (it used to be under a tab of its provider,
        // so with that tab gone it must be listed here). One that has an account is under that account.
        let accounts = vec!["a1".to_owned()];
        let k = Known { accounts: Some(&accounts) };
        assert_eq!(owner(&mark(None, false, 0), k), None);
        assert_eq!(owner(&mark(Some("a1"), false, 0), k), acct(Some("a1")));
        assert_eq!(owner(&mark(Some("deleted"), false, 0), k), None, "an account that was deleted: the default account");
        // What waits under it is counted for the choice like any chat's.
        let list = [mark(None, false, 2), mark(None, true, 0)];
        let seen = attention(&list, &acct(Some("a1")), k);
        assert_eq!(seen[&None], (1, 1));
    }

    #[test]
    fn what_waits_or_runs_is_counted_under_the_accounts_that_are_not_shown() {
        let accounts = vec!["a1".to_owned()];
        let k = Known { accounts: Some(&accounts) };
        let list = [mark(None, true, 2), mark(Some("a1"), true, 0), mark(Some("a1"), false, 1), mark(Some("a1"), false, 0), mark(Some("zz"), true, 0)];
        let seen = attention(&list, &acct(None), k);
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[&acct(Some("a1"))], (1, 1));
        let other = attention(&list, &acct(Some("a1")), k);
        assert_eq!(other[&acct(None)], (1, 1), "the deleted account's chat is under the default account");
        assert!(attention(&[], &acct(None), Known::default()).is_empty());
        assert_eq!(tab_label("Work", seen.get(&acct(Some("a1")))), "⚠ 1 Work");
        assert_eq!(tab_label("Work", Some(&(0, 3))), "● Work");
        assert_eq!(tab_label("Work", None), "Work");
    }

    #[test]
    fn an_archived_chat_is_hidden_unless_shown_running_waiting_or_open() {
        let chat = |archived, running, pending| Mark { account: None, running, pending, archived };
        assert!(listed(&chat(false, false, 0), false, false), "a chat that is not archived is listed");
        assert!(!listed(&chat(true, false, 0), false, false), "an archived one is not");
        assert!(listed(&chat(true, false, 0), true, false), "unless the archive is shown");
        assert!(listed(&chat(true, true, 0), false, false), "or it is working");
        assert!(listed(&chat(true, false, 2), false, false), "or it waits for the person");
        assert!(listed(&chat(true, false, 0), false, true), "or it is the open one");
    }

    #[test]
    fn the_account_choice_lists_only_accounts_each_with_what_waits_under_it() {
        let ids = vec![None, Some("a1".to_owned()), Some("a2".to_owned())];
        let names = vec!["Default".to_owned(), "Work".to_owned(), "Lab".to_owned()];
        let mut seen = HashMap::new();
        seen.insert(Some("a1".to_owned()), (2, 0));
        seen.insert(Some("a2".to_owned()), (0, 1));
        let entries = labels(&ids, &names, &seen);
        assert_eq!(entries, ["Default", "⚠ 2 Work", "● Lab"], "one entry per account, no provider among them");
        assert!(entries.iter().all(|e| !e.contains('→')));
        assert_eq!(labels(&[], &[], &HashMap::new()), Vec::<String>::new());
    }

    #[test]
    fn the_badge_beside_the_closed_choice_sums_what_waits_elsewhere() {
        let mut seen: HashMap<View, (usize, usize)> = HashMap::new();
        assert_eq!(away(&seen), "", "nothing elsewhere: no badge");
        seen.insert(Some("a1".to_owned()), (0, 2));
        assert_eq!(away(&seen), "● 2");
        seen.insert(Some("a2".to_owned()), (1, 0));
        seen.insert(None, (2, 5));
        assert_eq!(away(&seen), "⚠ 3", "waiting comes before working and the waiting chats are added up");
    }

    #[test]
    fn a_codex_account_is_a_row_but_never_an_entry_of_the_picker_and_says_how_it_is_logged_in() {
        let list = serde_json::json!({"accounts": [
            {"id": "cx", "name": "Codex Work", "kind": "codex", "open": true, "login": "CODEX_HOME=x codex login", "dir": "/d/cx"},
            {"id": "cl", "name": "Claude Work", "open": true, "login": "l", "dir": "/d/cl"},
        ]});
        let mut status = HashMap::new();
        status.insert("cx".to_owned(), Some(serde_json::json!({"kind": "codex", "status": {"logged_in": true, "method": "chatgpt", "known": true, "file": true}})));
        let (head, rows) = accounts_view(&list, &status, &HashMap::new(), Lang::En);
        assert_eq!(head.names, vec!["Default account".to_owned(), "Claude Work".to_owned()], "a chat cannot be made in a Codex account");
        assert_eq!(head.ids, vec![None, Some("cl".to_owned())]);
        let kinds: Vec<(&str, &str)> = rows.iter().map(|r| (r.id.as_str(), r.kind.as_str())).collect();
        assert_eq!(kinds, vec![("", "claude"), ("cl", "claude"), ("cx", "codex")], "Claude accounts first, then Codex ones");
        let cx = &rows[2];
        assert_eq!((cx.state.as_str(), cx.sub.as_str()), ("in", "Logged in with ChatGPT"));
        // A login outside the account's folder, one that is out, and an answer that could not be read.
        let ask = |status: serde_json::Value| {
            let mut m = HashMap::new();
            m.insert("cx".to_owned(), Some(serde_json::json!({"kind": "codex", "status": status})));
            accounts_view(&list, &m, &HashMap::new(), Lang::En).1[2].clone()
        };
        let outside = ask(serde_json::json!({"logged_in": true, "method": "api_key", "known": true, "file": false}));
        assert!(outside.sub.contains("API key") && outside.sub.contains("outside"), "{}", outside.sub);
        assert_eq!(ask(serde_json::json!({"logged_in": false, "method": null, "known": true, "file": false})).state, "out");
        assert_eq!(ask(serde_json::json!({"logged_in": false, "method": null, "known": false, "file": false})).state, "error");
    }
}
