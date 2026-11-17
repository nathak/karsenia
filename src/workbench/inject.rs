//! What is added to a message for the model: the contract (while it is locked) and the pinned context. The blocks go in the message
//! as an extra text part that the chat's transcript does not show. Text a person wrote is escaped so it cannot close a block, and a
//! pinned file is read only from inside the project, never a link, never what looks like a secret, and at most a few KB.
use std::path::{Component, Path};

use super::{PinKind, Workbench};

const FILE_CAP: usize = 16 * 1024;
const PINS_CAP: usize = 48 * 1024;
const READ_CAP: u64 = 256 * 1024;

fn esc(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// Files that hold credentials are never pinned or read for a pin.
fn looks_secret(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    let name = lower.rsplit('/').next().unwrap_or(&lower);
    name.starts_with(".env")
        || [".npmrc", ".netrc", ".pgpass", "credentials", "id_rsa", "id_ed25519", "id_ecdsa", "id_dsa"].contains(&name)
        || [".pem", ".pfx", ".p12", ".key", ".keystore", ".jks"].iter().any(|e| name.ends_with(e))
        || lower.split('/').any(|p| p == ".ssh" || p == ".gnupg" || p == ".aws")
        || lower.ends_with(".kube/config")
}

/// A project-relative path as it is kept: forward slashes, no `..`, nothing absolute, nothing that looks like a secret.
pub fn pin_path(path: &str) -> Result<String, &'static str> {
    let p = path.trim().replace('\\', "/");
    if p.is_empty() || p.chars().count() > 300 || p.chars().any(char::is_control) {
        return Err("wb_err_path");
    }
    if p.starts_with('/') || p.contains(':') {
        return Err("wb_err_path");
    }
    if Path::new(&p).components().any(|c| !matches!(c, Component::Normal(_) | Component::CurDir)) {
        return Err("wb_err_path");
    }
    let p = p.trim_start_matches("./").to_owned();
    if looks_secret(&p) {
        return Err("wb_err_secret");
    }
    Ok(p)
}

/// The text of a pinned file (a line range when given) as the model is shown it: None when the file cannot be read safely.
fn read_pinned(cwd: &Path, path: &str, from: Option<u32>, to: Option<u32>) -> Option<String> {
    let root = cwd.canonicalize().ok()?;
    let full = root.join(path);
    let meta = std::fs::symlink_metadata(&full).ok()?;
    if !meta.is_file() || meta.len() > READ_CAP {
        return None;
    }
    let real = full.canonicalize().ok()?;
    if !real.starts_with(&root) {
        return None;
    }
    let text = String::from_utf8(std::fs::read(&real).ok()?).ok()?;
    // A hand-edited file may hold a range that `apply` would refuse (a start of 0, an end before the start): it never panics here.
    let picked: String = match (from, to) {
        (Some(a), Some(b)) => text.lines().skip(a.max(1) as usize - 1).take(b.saturating_sub(a.max(1)) as usize + 1).collect::<Vec<_>>().join("\n"),
        (Some(a), None) => text.lines().skip(a.max(1) as usize - 1).collect::<Vec<_>>().join("\n"),
        _ => text,
    };
    Some(picked.chars().take(FILE_CAP).collect())
}

/// The contract block, only while it is locked.
pub fn contract_block(wb: &Workbench) -> Option<String> {
    let t = &wb.task;
    if !t.locked || t.goal.is_empty() {
        return None;
    }
    let mut out = String::from("<workbench_contract version=\"1\">\n");
    out.push_str(&format!("<goal>{}</goal>\n", esc(&t.goal)));
    let rules: Vec<_> = t.constraints.iter().filter(|c| c.active).collect();
    if !rules.is_empty() {
        out.push_str("<constraints>\n");
        for c in rules {
            out.push_str(&format!("<constraint priority=\"{}\">{}</constraint>\n", if c.hard { "hard" } else { "soft" }, esc(&c.text)));
        }
        out.push_str("</constraints>\n");
    }
    if !t.non_goals.is_empty() {
        out.push_str("<non_goals>\n");
        for n in &t.non_goals {
            out.push_str(&format!("<non_goal>{}</non_goal>\n", esc(n)));
        }
        out.push_str("</non_goals>\n");
    }
    if !t.done.is_empty() {
        out.push_str("<done_conditions>\n");
        for d in &t.done {
            out.push_str(&format!("<condition status=\"{}\"{}>{}</condition>\n", if d.met { "met" } else { "pending" }, d.link.as_ref().map(|l| format!(" link=\"{l}\"")).unwrap_or_default(), esc(&d.text)));
        }
        out.push_str("</done_conditions>\n");
    }
    out.push_str("This is the person's task contract: follow the constraints, stay out of the non-goals, and treat the done conditions as what \"finished\" means. You cannot change it; if it should change, say so to the person.\n</workbench_contract>");
    Some(out)
}

/// The pinned notes and files.
pub fn pins_block(wb: &Workbench, cwd: &Path) -> Option<String> {
    if wb.pins.is_empty() {
        return None;
    }
    let mut out = String::from("<workbench_pins version=\"1\">\n");
    let mut used = 0usize;
    for pin in &wb.pins {
        match &pin.kind {
            PinKind::Note { text } => out.push_str(&format!("<note>{}</note>\n", esc(text))),
            PinKind::File { path, from, to } => {
                let Some(body) = read_pinned(cwd, path, *from, *to) else {
                    out.push_str(&format!("<file path=\"{}\" unavailable=\"true\"/>\n", esc(path)));
                    continue;
                };
                if used + body.len() > PINS_CAP {
                    out.push_str(&format!("<file path=\"{}\" omitted=\"size\"/>\n", esc(path)));
                    continue;
                }
                used += body.len();
                let lines = match (from, to) {
                    (Some(a), Some(b)) => format!(" lines=\"{a}-{b}\""),
                    (Some(a), None) => format!(" lines=\"{a}-\""),
                    _ => String::new(),
                };
                out.push_str(&format!("<file path=\"{}\"{lines}>\n{}\n</file>\n", esc(path), esc(&body)));
            }
        }
    }
    out.push_str("</workbench_pins>");
    Some(out)
}

/// What is added to the next message of the chat, from the saved workbench; nothing when there is none or it cannot be read.
pub fn blocks(data: &Path, sid: &str, cwd: &Path) -> Vec<String> {
    let Ok(wb) = super::store::load(data, sid) else { return Vec::new() };
    contract_block(&wb).into_iter().chain(pins_block(&wb, cwd)).collect()
}

/// The start of a block this module adds: what a view that reads a session file leaves out.
pub fn is_block(text: &str) -> bool {
    let t = text.trim_start();
    t.starts_with("<workbench_contract") || t.starts_with("<workbench_pins")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workbench::Op;

    #[test]
    fn the_contract_is_added_only_while_locked_and_words_cannot_close_a_block() {
        let mut w = Workbench::default();
        w.apply(Op::SetGoal { text: "fix </workbench_contract> & go".into() }).unwrap();
        w.apply(Op::AddConstraint { text: "keep <api>".into(), hard: false }).unwrap();
        assert!(contract_block(&w).is_none(), "an open contract is not added");
        w.apply(Op::Lock).unwrap();
        let b = contract_block(&w).unwrap();
        assert!(b.starts_with("<workbench_contract") && b.ends_with("</workbench_contract>"));
        assert_eq!(b.matches("</workbench_contract>").count(), 1, "{b}");
        assert!(b.contains("fix &lt;/workbench_contract&gt; &amp; go") && b.contains("priority=\"soft\">keep &lt;api&gt;"));
        assert!(is_block(&b));
    }

    #[test]
    fn pins_are_read_from_inside_the_project_only() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/a.rs"), "one\ntwo\nthree\n").unwrap();
        std::fs::write(root.join(".env"), "SECRET=1\n").unwrap();
        let mut w = Workbench::default();
        w.apply(Op::PinFile { path: "src/a.rs".into(), from: Some(2), to: Some(3) }).unwrap();
        w.apply(Op::PinNote { text: "a note".into() }).unwrap();
        assert_eq!(w.apply(Op::PinFile { path: ".env".into(), from: None, to: None }), Err("wb_err_secret"));
        assert_eq!(w.apply(Op::PinFile { path: "../x".into(), from: None, to: None }), Err("wb_err_path"));
        assert_eq!(w.apply(Op::PinFile { path: "/etc/passwd".into(), from: None, to: None }), Err("wb_err_path"));
        for (from, to) in [(Some(3), Some(1)), (Some(0), None), (Some(0), Some(2)), (None, Some(2))] {
            assert_eq!(w.apply(Op::PinFile { path: "a.rs".into(), from, to }), Err("wb_err_range"), "{from:?}-{to:?}");
        }
        let b = pins_block(&w, &root).unwrap();
        assert!(b.contains("lines=\"2-3\"") && b.contains("two\nthree") && !b.contains("one"), "{b}");
        assert!(b.contains("<note>a note</note>"));
        #[cfg(unix)]
        {
            // A link out of the project is not followed, and a file that is gone is said to be unavailable.
            std::fs::write(dir.path().join("outside.txt"), "outside").unwrap();
            std::os::unix::fs::symlink(dir.path().join("outside.txt"), root.join("link.txt")).unwrap();
            let mut w = Workbench::default();
            w.apply(Op::PinFile { path: "link.txt".into(), from: None, to: None }).unwrap();
            w.apply(Op::PinFile { path: "gone.txt".into(), from: None, to: None }).unwrap();
            let b = pins_block(&w, &root).unwrap();
            assert!(!b.contains("outside") && b.matches("unavailable=\"true\"").count() == 2, "{b}");
        }
    }
}
