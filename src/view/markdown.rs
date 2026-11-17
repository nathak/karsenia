//! A model's markdown split into blocks the desktop app draws one by one. Slint's own markdown (`StyledText::from_markdown`)
//! refuses headings, quotes, code blocks and tables, and reads some inline HTML (`<font color>`, `<u>`), so the text is never
//! handed to it as it came: blocks are made here, and the inline part of each block is written out again with every ASCII
//! punctuation mark escaped and only bold, italic, strike-through, inline code and http(s) links put back on.
use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    /// Inline markdown safe for `StyledText::from_markdown`.
    Para(String),
    Heading(u8, String),
    /// A list item: its nesting depth (0 is the outer list), the marker to draw (`•`, `3.`, `☐`, `☑`) and its inline markdown.
    Item { depth: u8, marker: String, md: String },
    Quote(String),
    Code { lang: String, text: String },
    Rule,
    /// A table: as plain text (one line per row, cells joined with ` │ `, for copying) and as rows of cells (the first row is the header).
    Table { text: String, grid: Vec<Vec<String>> },
}

/// Escapes every ASCII punctuation mark, so nothing in `text` is read as markdown or HTML.
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 8);
    for c in text.chars() {
        if c.is_ascii_punctuation() {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Inline code: a fence one backtick longer than any run inside, with spaces when the code starts or ends with a backtick.
fn code_span(code: &str) -> String {
    let longest = code.split(|c| c != '`').map(str::len).max().unwrap_or(0);
    let fence = "`".repeat(longest + 1);
    let pad = if code.starts_with('`') || code.ends_with('`') || code.is_empty() { " " } else { "" };
    format!("{fence}{pad}{code}{pad}{fence}")
}

fn safe_url(url: &str) -> Option<String> {
    let lower = url.trim().to_ascii_lowercase();
    if !(lower.starts_with("https://") || lower.starts_with("http://")) {
        return None;
    }
    Some(url.trim().replace('\\', "%5C").replace(' ', "%20").replace('(', "%28").replace(')', "%29").replace('<', "%3C").replace('>', "%3E"))
}

/// Where the inline text of the block being read goes.
#[derive(Default)]
struct Inline {
    md: String,
    /// Open links: the safe address, or None for an address that is not http(s) (its text stays, the link does not).
    links: Vec<Option<String>>,
}

impl Inline {
    fn take(&mut self) -> String {
        self.links.clear();
        std::mem::take(&mut self.md).trim().to_owned()
    }
}

/// The blocks of `src`. Never fails: whatever cannot be drawn becomes plain escaped text.
pub fn blocks(src: &str) -> Vec<Block> {
    let mut out = Vec::new();
    let mut inline = Inline::default();
    // Lists: one entry per open list, the next number for an ordered one.
    let mut lists: Vec<Option<u64>> = Vec::new();
    let mut item_marker: Option<String> = None;
    let mut heading: Option<u8> = None;
    let mut quote = 0usize;
    let mut code: Option<(String, String)> = None;
    let mut table: Option<(Vec<Vec<String>>, Vec<String>)> = None;

    // A block ends: what was read becomes the block its context says.
    fn flush(out: &mut Vec<Block>, inline: &mut Inline, heading: Option<u8>, quote: usize, lists: &[Option<u64>], marker: &mut Option<String>) {
        let md = inline.take();
        if md.is_empty() && marker.is_none() {
            return;
        }
        if let Some(level) = heading {
            out.push(Block::Heading(level, md));
        } else if let Some(m) = marker.take() {
            out.push(Block::Item { depth: lists.len().saturating_sub(1).min(8) as u8, marker: m, md });
        } else if quote > 0 {
            out.push(Block::Quote(md));
        } else {
            out.push(Block::Para(md));
        }
    }

    let options = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    for event in Parser::new_ext(src, options) {
        if let Some((_, text)) = code.as_mut() {
            match event {
                Event::Text(t) => text.push_str(&t),
                Event::End(TagEnd::CodeBlock) => {
                    let (lang, text) = code.take().unwrap_or_default();
                    out.push(Block::Code { lang, text: text.trim_end_matches('\n').to_owned() });
                }
                _ => {}
            }
            continue;
        }
        if let Some((rows, cells)) = table.as_mut() {
            match event {
                Event::Text(t) | Event::Code(t) => {
                    if let Some(last) = cells.last_mut() {
                        last.push_str(&t);
                    }
                }
                Event::Start(Tag::TableCell) => cells.push(String::new()),
                Event::End(TagEnd::TableHead) | Event::End(TagEnd::TableRow) => rows.push(std::mem::take(cells)),
                Event::End(TagEnd::Table) => {
                    let (grid, _) = table.take().unwrap_or_default();
                    let text = grid.iter().map(|row| row.join(" │ ")).collect::<Vec<_>>().join("\n");
                    out.push(Block::Table { text, grid });
                }
                _ => {}
            }
            continue;
        }
        match event {
            Event::Start(Tag::CodeBlock(kind)) => {
                flush(&mut out, &mut inline, heading, quote, &lists, &mut item_marker);
                let lang = match kind {
                    CodeBlockKind::Fenced(info) => info.split_whitespace().next().unwrap_or("").chars().filter(|c| c.is_ascii_alphanumeric() || "+-#_.".contains(*c)).take(20).collect(),
                    CodeBlockKind::Indented => String::new(),
                };
                code = Some((lang, String::new()));
            }
            Event::Start(Tag::Table(_)) => {
                flush(&mut out, &mut inline, heading, quote, &lists, &mut item_marker);
                table = Some((Vec::new(), Vec::new()));
            }
            Event::Start(Tag::Heading { level, .. }) => {
                flush(&mut out, &mut inline, heading, quote, &lists, &mut item_marker);
                heading = Some(match level {
                    HeadingLevel::H1 => 1,
                    HeadingLevel::H2 => 2,
                    HeadingLevel::H3 => 3,
                    _ => 4,
                });
            }
            Event::End(TagEnd::Heading(_)) => {
                flush(&mut out, &mut inline, heading, quote, &lists, &mut item_marker);
                heading = None;
            }
            Event::Start(Tag::BlockQuote(_)) => {
                flush(&mut out, &mut inline, heading, quote, &lists, &mut item_marker);
                quote += 1;
            }
            Event::End(TagEnd::BlockQuote(_)) => {
                flush(&mut out, &mut inline, heading, quote, &lists, &mut item_marker);
                quote = quote.saturating_sub(1);
            }
            Event::Start(Tag::List(start)) => {
                flush(&mut out, &mut inline, heading, quote, &lists, &mut item_marker);
                lists.push(start);
            }
            Event::End(TagEnd::List(_)) => {
                flush(&mut out, &mut inline, heading, quote, &lists, &mut item_marker);
                lists.pop();
            }
            Event::Start(Tag::Item) => {
                flush(&mut out, &mut inline, heading, quote, &lists, &mut item_marker);
                item_marker = Some(match lists.last_mut() {
                    Some(Some(n)) => {
                        let m = format!("{n}.");
                        *n += 1;
                        m
                    }
                    _ => "•".to_owned(),
                });
            }
            Event::TaskListMarker(done) => item_marker = Some(if done { "☑" } else { "☐" }.to_owned()),
            Event::End(TagEnd::Item) | Event::End(TagEnd::Paragraph) => flush(&mut out, &mut inline, heading, quote, &lists, &mut item_marker),
            Event::Start(Tag::Paragraph) => {
                // A second paragraph of one list item is its own line, without a marker.
                if item_marker.is_none() && !inline.md.trim().is_empty() {
                    flush(&mut out, &mut inline, heading, quote, &lists, &mut item_marker);
                }
            }
            Event::Rule => {
                flush(&mut out, &mut inline, heading, quote, &lists, &mut item_marker);
                out.push(Block::Rule);
            }
            Event::Start(Tag::Strong) | Event::End(TagEnd::Strong) => inline.md.push_str("**"),
            Event::Start(Tag::Emphasis) | Event::End(TagEnd::Emphasis) => inline.md.push('*'),
            Event::Start(Tag::Strikethrough) | Event::End(TagEnd::Strikethrough) => inline.md.push_str("~~"),
            Event::Start(Tag::Link { dest_url, .. }) => {
                let url = safe_url(&dest_url);
                if url.is_some() {
                    inline.md.push('[');
                }
                inline.links.push(url);
            }
            Event::End(TagEnd::Link) => {
                if let Some(Some(url)) = inline.links.pop() {
                    inline.md.push_str("](");
                    inline.md.push_str(&url);
                    inline.md.push(')');
                }
            }
            // An image is not drawn: its alternative text stays as text.
            Event::Start(Tag::Image { .. }) | Event::End(TagEnd::Image) => {}
            Event::Text(t) => inline.md.push_str(&escape(&t)),
            Event::Code(t) => inline.md.push_str(&code_span(&t)),
            // HTML, math and footnotes are shown as the text they are, never read.
            Event::Html(t) | Event::InlineHtml(t) | Event::InlineMath(t) | Event::DisplayMath(t) | Event::FootnoteReference(t) => inline.md.push_str(&escape(&t)),
            Event::SoftBreak => inline.md.push(' '),
            Event::HardBreak => inline.md.push('\n'),
            _ => {}
        }
    }
    if let Some((lang, text)) = code {
        out.push(Block::Code { lang, text: text.trim_end_matches('\n').to_owned() });
    }
    flush(&mut out, &mut inline, heading, quote, &lists, &mut item_marker);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_and_odd_links_stay_text() {
        let b = blocks("hi <font color=red>x</font> <u>u</u> [bad](javascript:alert(1)) [ok](https://a.example/p(1))");
        let Block::Para(md) = &b[0] else { panic!("{b:?}") };
        assert!(md.contains(r"\<font color\=red\>"), "{md}");
        assert!(md.contains(r"\<u\>u\<\/u\>"), "{md}");
        assert!(!md.contains("javascript"), "a link that is not http(s) keeps only its text: {md}");
        assert!(md.contains("[ok](https://a.example/p%281%29)"), "{md}");
    }

    #[test]
    fn blocks_are_split() {
        let b = blocks("# Title\n\npara **b** *i* ~~s~~ `c`\n\n- a\n  - b\n1. one\n2. two\n- [x] done\n\n> q\n\n```js\nconsole.log(1)\n```\n\n---\n\n| a | b |\n|---|---|\n| 1 | 2 |\n");
        assert_eq!(b[0], Block::Heading(1, "Title".into()));
        assert_eq!(b[1], Block::Para("para **b** *i* ~~s~~ `c`".into()));
        assert_eq!(b[2], Block::Item { depth: 0, marker: "•".into(), md: "a".into() });
        assert_eq!(b[3], Block::Item { depth: 1, marker: "•".into(), md: "b".into() });
        assert_eq!(b[4], Block::Item { depth: 0, marker: "1.".into(), md: "one".into() });
        assert_eq!(b[5], Block::Item { depth: 0, marker: "2.".into(), md: "two".into() });
        assert_eq!(b[6], Block::Item { depth: 0, marker: "☑".into(), md: "done".into() });
        assert_eq!(b[7], Block::Quote("q".into()));
        assert_eq!(b[8], Block::Code { lang: "js".into(), text: "console.log(1)".into() });
        assert_eq!(b[9], Block::Rule);
        assert_eq!(b[10], Block::Table { text: "a │ b\n1 │ 2".into(), grid: vec![vec!["a".into(), "b".into()], vec!["1".into(), "2".into()]] });
    }

    #[test]
    fn an_open_fence_is_still_code_and_backticks_in_code_are_kept() {
        assert_eq!(blocks("```\nlet a = 1;"), vec![Block::Code { lang: String::new(), text: "let a = 1;".into() }]);
        let b = blocks("x ``a`b`` y");
        let Block::Para(md) = &b[0] else { panic!() };
        assert!(md.contains("``a`b``"), "{md}");
    }
}
