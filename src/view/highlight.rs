//! Syntax colours for the file viewer, worked out without any UI toolkit. A small hand-written scanner (no crate): comments,
//! strings, numbers, keywords and a few language specials, per line, with the state that runs over a line break (a block
//! comment, a triple-quoted or raw string). It colours what an editor's simple mode does and nothing more: a wrong guess
//! only changes a colour, never the text. The tokens of a line put together are the line itself (without trailing blanks).
//!
//! Layout rules the drawing relies on: a blank run is never a token of its own (it is the start of the next one), so every
//! token starts with the text it is drawn for; neighbouring tokens of one kind are one token.

use super::files::MAX_GUTTER;

/// What a path shows as in the file viewer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Markdown,
    Code(Lang),
    /// Anything else: the editor as it was.
    Plain,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Rust,
    Java,
    Kotlin,
    Python,
    JsTs,
    CLike,
    CSharp,
    Go,
    Json,
    Toml,
    Yaml,
    Shell,
    Markup,
    Css,
    Sql,
    Slint,
}

/// The view a file name gets, by its extension (case does not matter; both kinds of path separator end a folder name).
pub fn view_of(path: &str) -> View {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let Some((stem, ext)) = name.rsplit_once('.') else { return View::Plain };
    if stem.is_empty() {
        return View::Plain;
    }
    match ext.to_ascii_lowercase().as_str() {
        "md" | "markdown" | "mdx" => View::Markdown,
        "rs" => View::Code(Lang::Rust),
        "java" => View::Code(Lang::Java),
        "kt" | "kts" => View::Code(Lang::Kotlin),
        "py" | "pyw" => View::Code(Lang::Python),
        "js" | "mjs" | "cjs" | "jsx" | "ts" | "tsx" => View::Code(Lang::JsTs),
        "c" | "h" | "cc" | "cpp" | "cxx" | "hpp" | "hh" | "m" => View::Code(Lang::CLike),
        "cs" => View::Code(Lang::CSharp),
        "go" => View::Code(Lang::Go),
        "json" | "jsonc" => View::Code(Lang::Json),
        "toml" => View::Code(Lang::Toml),
        "yml" | "yaml" => View::Code(Lang::Yaml),
        "sh" | "bash" | "zsh" => View::Code(Lang::Shell),
        "html" | "htm" | "xml" | "svg" | "xaml" => View::Code(Lang::Markup),
        "css" | "scss" => View::Code(Lang::Css),
        "sql" => View::Code(Lang::Sql),
        "slint" => View::Code(Lang::Slint),
        _ => View::Plain,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Plain,
    Keyword,
    Type,
    Str,
    Comment,
    Number,
    Punct,
    Meta,
}

impl Kind {
    /// The number the window maps to a colour.
    pub fn code(self) -> i32 {
        match self {
            Kind::Plain => 0,
            Kind::Keyword => 1,
            Kind::Type => 2,
            Kind::Str => 3,
            Kind::Comment => 4,
            Kind::Number => 5,
            Kind::Punct => 6,
            Kind::Meta => 7,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tok {
    pub text: String,
    pub kind: Kind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    /// The line number, from 1.
    pub n: usize,
    pub toks: Vec<Tok>,
}

/// A line longer than this is one plain token (a minified file would make a layout of thousands of pieces).
pub const MAX_LINE_CHARS: usize = 2000;
const TAB: usize = 4;

/// How many columns a character takes in a monospace face (wide East Asian characters and pictographs take two).
pub fn char_cols(c: char) -> usize {
    match u32::from(c) {
        0x1100..=0x115F | 0x2E80..=0xA4CF | 0xAC00..=0xD7A3 | 0xF900..=0xFAFF | 0xFE30..=0xFE6F | 0xFF00..=0xFF60 | 0xFFE0..=0xFFE6 | 0x1F300..=0x1F64F | 0x20000..=0x3FFFD => 2,
        _ => 1,
    }
}

/// The lines of a text as they are drawn: no byte order mark, no `\r` at a line end, tabs spread to the next multiple of four columns.
pub fn display_lines(text: &str) -> Vec<String> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    text.split('\n')
        .map(|line| {
            let line = line.strip_suffix('\r').unwrap_or(line);
            let mut out = String::with_capacity(line.len());
            let mut col = 0;
            for c in line.chars() {
                if c == '\t' {
                    let fill = TAB - col % TAB;
                    out.extend(std::iter::repeat_n(' ', fill));
                    col += fill;
                } else {
                    out.push(c);
                    col += char_cols(c);
                }
            }
            out
        })
        .collect()
}

/// The coloured lines of a text and the widest line in columns, or None when the text is too long to be worth it (the editor is used then).
pub fn highlight(lang: Lang, text: &str) -> Option<(Vec<Line>, usize)> {
    let lines = display_lines(text);
    if lines.len() > MAX_GUTTER {
        return None;
    }
    let spec = spec(lang);
    let mut carry = Carry::None;
    let mut in_tag = false;
    let mut widest = 0;
    let mut out = Vec::with_capacity(lines.len());
    for (i, line) in lines.iter().enumerate() {
        widest = widest.max(line.chars().map(char_cols).sum());
        let toks = if line.chars().count() > MAX_LINE_CHARS {
            let text = line.trim_end();
            if text.is_empty() { Vec::new() } else { vec![Tok { text: text.to_owned(), kind: Kind::Plain }] }
        } else {
            let mut scan = Scan { spec: &spec, lang, cs: line.chars().collect(), i: 0, out: Vec::new(), carry, in_tag };
            scan.run();
            carry = scan.carry;
            in_tag = scan.in_tag;
            join(scan.out)
        };
        out.push(Line { n: i + 1, toks });
    }
    Some((out, widest))
}

/// Blanks go to the front of the next token, blanks at the end of the line are dropped, neighbours of one kind become one token.
fn join(raw: Vec<(Kind, String)>) -> Vec<Tok> {
    let mut out: Vec<Tok> = Vec::new();
    let mut blanks = String::new();
    for (kind, text) in raw {
        if text.chars().all(char::is_whitespace) {
            blanks.push_str(&text);
            continue;
        }
        let text = format!("{}{}", std::mem::take(&mut blanks), text);
        match out.last_mut() {
            Some(last) if last.kind == kind => last.text.push_str(&text),
            _ => out.push(Tok { text, kind }),
        }
    }
    // A comment or a string that runs to the end of the line carries the blanks after it.
    if let Some(last) = out.last_mut() {
        last.text.truncate(last.text.trim_end().len());
        if last.text.is_empty() {
            out.pop();
        }
    }
    out
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Carry {
    None,
    Block { depth: u32 },
    Str { quote: char, triple: bool },
    RawStr { hashes: usize },
}

struct Spec {
    keywords: &'static [&'static str],
    line: &'static [&'static str],
    block: Option<(&'static str, &'static str)>,
    nested: bool,
    quotes: &'static [char],
    triple: bool,
    /// A name that starts with a capital and has a small letter in it is a type.
    types: bool,
    /// Keywords match whatever the case.
    nocase: bool,
}

const C_LIKE: &[&str] = &["if", "else", "for", "while", "do", "switch", "case", "default", "break", "continue", "return", "goto", "struct", "union", "enum", "typedef", "static", "const", "extern", "volatile", "sizeof", "void", "int", "long", "short", "char", "float", "double", "unsigned", "signed", "bool", "class", "public", "private", "protected", "virtual", "namespace", "using", "template", "typename", "new", "delete", "try", "catch", "throw", "nullptr", "true", "false", "this", "auto", "inline", "operator", "include", "define", "ifdef", "ifndef", "endif"];

fn spec(lang: Lang) -> Spec {
    let base = Spec { keywords: &[], line: &["//"], block: Some(("/*", "*/")), nested: false, quotes: &['"', '\''], triple: false, types: true, nocase: false };
    match lang {
        Lang::Rust => Spec { keywords: &["as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern", "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub", "ref", "return", "self", "Self", "static", "struct", "super", "trait", "true", "type", "unsafe", "use", "where", "while", "bool", "char", "str", "u8", "u16", "u32", "u64", "u128", "usize", "i8", "i16", "i32", "i64", "i128", "isize", "f32", "f64"], nested: true, ..base },
        Lang::Java => Spec { keywords: &["abstract", "assert", "boolean", "break", "byte", "case", "catch", "char", "class", "const", "continue", "default", "do", "double", "else", "enum", "extends", "final", "finally", "float", "for", "goto", "if", "implements", "import", "instanceof", "int", "interface", "long", "native", "new", "package", "private", "protected", "public", "return", "short", "static", "strictfp", "super", "switch", "synchronized", "this", "throw", "throws", "transient", "try", "void", "volatile", "while", "true", "false", "null", "var", "record", "sealed", "yield"], triple: true, ..base },
        Lang::Kotlin => Spec { keywords: &["as", "break", "class", "continue", "do", "else", "false", "for", "fun", "if", "in", "interface", "is", "null", "object", "package", "return", "super", "this", "throw", "true", "try", "typealias", "val", "var", "when", "while", "by", "catch", "constructor", "data", "enum", "finally", "import", "init", "inner", "internal", "lateinit", "open", "override", "private", "protected", "public", "sealed", "suspend", "companion", "abstract", "final", "const"], nested: true, triple: true, ..base },
        Lang::Python => Spec { keywords: &["and", "as", "assert", "async", "await", "break", "class", "continue", "def", "del", "elif", "else", "except", "finally", "for", "from", "global", "if", "import", "in", "is", "lambda", "nonlocal", "not", "or", "pass", "raise", "return", "try", "while", "with", "yield", "None", "True", "False", "self", "match", "case"], line: &["#"], block: None, triple: true, ..base },
        Lang::JsTs => Spec { keywords: &["async", "await", "break", "case", "catch", "class", "const", "continue", "debugger", "default", "delete", "do", "else", "export", "extends", "false", "finally", "for", "function", "if", "import", "in", "instanceof", "let", "new", "null", "of", "return", "static", "super", "switch", "this", "throw", "true", "try", "typeof", "undefined", "var", "void", "while", "with", "yield", "interface", "type", "enum", "implements", "readonly", "public", "private", "protected", "as", "from", "number", "string", "boolean", "any", "unknown", "never"], quotes: &['"', '\'', '`'], ..base },
        Lang::CLike => Spec { keywords: C_LIKE, ..base },
        Lang::CSharp => Spec { keywords: &["abstract", "as", "base", "bool", "break", "byte", "case", "catch", "char", "class", "const", "continue", "decimal", "default", "do", "double", "else", "enum", "false", "finally", "float", "for", "foreach", "if", "in", "int", "interface", "internal", "is", "lock", "long", "namespace", "new", "null", "object", "out", "override", "private", "protected", "public", "readonly", "ref", "return", "sealed", "short", "static", "string", "struct", "switch", "this", "throw", "true", "try", "typeof", "using", "var", "virtual", "void", "while", "async", "await", "get", "set"], triple: true, ..base },
        Lang::Go => Spec { keywords: &["break", "case", "chan", "const", "continue", "default", "defer", "else", "fallthrough", "for", "func", "go", "goto", "if", "import", "interface", "map", "package", "range", "return", "select", "struct", "switch", "type", "var", "true", "false", "nil", "string", "int", "int64", "uint", "byte", "bool", "error", "float64"], quotes: &['"', '\'', '`'], ..base },
        Lang::Json => Spec { keywords: &["true", "false", "null"], line: &[], block: None, quotes: &['"'], types: false, ..base },
        Lang::Toml => Spec { keywords: &["true", "false"], line: &["#"], block: None, quotes: &['"', '\''], types: false, triple: true, ..base },
        Lang::Yaml => Spec { keywords: &["true", "false", "null", "yes", "no", "on", "off"], line: &["#"], block: None, quotes: &['"', '\''], types: false, ..base },
        Lang::Shell => Spec { keywords: &["if", "then", "else", "elif", "fi", "for", "while", "until", "do", "done", "case", "esac", "in", "function", "return", "exit", "export", "local", "readonly", "set", "unset", "source", "echo", "cd", "true", "false"], line: &["#"], block: None, quotes: &['"', '\''], types: false, ..base },
        Lang::Markup => Spec { keywords: &[], line: &[], block: Some(("<!--", "-->")), quotes: &[], types: false, ..base },
        Lang::Css => Spec { keywords: &["important"], line: &[], quotes: &['"', '\''], types: false, ..base },
        Lang::Sql => Spec { keywords: &["select", "from", "where", "and", "or", "not", "null", "insert", "into", "values", "update", "set", "delete", "create", "table", "drop", "alter", "add", "index", "join", "left", "right", "inner", "outer", "full", "on", "as", "group", "by", "order", "having", "limit", "offset", "distinct", "union", "all", "in", "is", "like", "between", "exists", "case", "when", "then", "else", "end", "primary", "key", "foreign", "references", "default", "unique", "constraint", "view", "begin", "commit", "rollback", "int", "integer", "text", "varchar", "boolean", "true", "false", "asc", "desc", "with"], line: &["--"], quotes: &['\'', '"'], types: false, nocase: true, ..base },
        Lang::Slint => Spec { keywords: &["component", "global", "struct", "enum", "export", "import", "from", "property", "in", "out", "in-out", "private", "callback", "function", "pure", "public", "if", "else", "for", "return", "animate", "states", "transitions", "when", "root", "self", "parent", "true", "false", "inherits", "init", "changed", "int", "float", "string", "bool", "length", "duration", "color", "brush", "image", "angle", "percent"], ..base },
    }
}

struct Scan<'a> {
    spec: &'a Spec,
    lang: Lang,
    cs: Vec<char>,
    i: usize,
    out: Vec<(Kind, String)>,
    carry: Carry,
    in_tag: bool,
}

impl Scan<'_> {
    fn starts(&self, at: usize, word: &str) -> bool {
        word.chars().enumerate().all(|(k, w)| self.cs.get(at + k) == Some(&w))
    }

    fn text(&self, from: usize, to: usize) -> String {
        self.cs[from..to].iter().collect()
    }

    fn push(&mut self, kind: Kind, from: usize, to: usize) {
        if to > from {
            self.out.push((kind, self.text(from, to)));
        }
    }

    fn ident_start(&self, c: char) -> bool {
        c.is_alphabetic() || c == '_' || (self.lang == Lang::JsTs && c == '$')
    }

    fn ident_char(&self, c: char) -> bool {
        c.is_alphanumeric() || c == '_' || (self.lang == Lang::JsTs && c == '$') || (c == '-' && matches!(self.lang, Lang::Css | Lang::Slint | Lang::Yaml | Lang::Toml | Lang::Markup))
    }

    fn prev_is_ident(&self) -> bool {
        self.i > 0 && (self.cs[self.i - 1].is_alphanumeric() || self.cs[self.i - 1] == '_')
    }

    /// The first non-blank character at or after `at`.
    fn next_sig(&self, at: usize) -> Option<char> {
        self.cs[at.min(self.cs.len())..].iter().copied().find(|c| !c.is_whitespace())
    }

    fn multiline(&self, quote: char, triple: bool) -> bool {
        triple || quote == '`' || (self.lang == Lang::Rust && quote == '"') || (self.lang == Lang::Markup)
    }

    fn run(&mut self) {
        // What the line before left open.
        match self.carry {
            Carry::Block { depth } => self.block_body(depth),
            Carry::Str { quote, triple } => self.string_body(quote, triple),
            Carry::RawStr { hashes } => self.raw_body(hashes),
            Carry::None => {}
        }
        if self.lang == Lang::Toml && self.carry == Carry::None && self.out.is_empty() && self.next_sig(self.i) == Some('[') {
            let (from, to) = (self.i, self.cs.len());
            self.push(Kind::Meta, from, to);
            return;
        }
        while self.i < self.cs.len() {
            self.step();
        }
    }

    /// From the character at `i` (inside a block comment of this depth) to its end, or the end of the line.
    fn block_body(&mut self, mut depth: u32) {
        let (open, close) = self.spec.block.unwrap_or(("", ""));
        let from = self.i;
        while self.i < self.cs.len() {
            if self.spec.nested && !open.is_empty() && self.starts(self.i, open) {
                depth += 1;
                self.i += open.chars().count();
            } else if !close.is_empty() && self.starts(self.i, close) {
                depth -= 1;
                self.i += close.chars().count();
                if depth == 0 {
                    self.push(Kind::Comment, from, self.i);
                    self.carry = Carry::None;
                    return;
                }
            } else {
                self.i += 1;
            }
        }
        self.push(Kind::Comment, from, self.i);
        self.carry = Carry::Block { depth };
    }

    /// From the character at `i` (inside a string) to its end, or the end of the line.
    fn string_body(&mut self, quote: char, triple: bool) {
        let from = self.i;
        let escapes = !(self.lang == Lang::Go && quote == '`');
        while self.i < self.cs.len() {
            let c = self.cs[self.i];
            if escapes && c == '\\' {
                self.i += 2;
                continue;
            }
            if c == quote && (!triple || (self.cs.get(self.i + 1) == Some(&quote) && self.cs.get(self.i + 2) == Some(&quote))) {
                self.i += if triple { 3 } else { 1 };
                self.push(Kind::Str, from, self.i.min(self.cs.len()));
                self.carry = Carry::None;
                self.key_check();
                return;
            }
            self.i += 1;
        }
        self.i = self.cs.len();
        self.push(Kind::Str, from, self.i);
        self.carry = if self.multiline(quote, triple) { Carry::Str { quote, triple } } else { Carry::None };
    }

    /// A JSON string followed by a colon is a key.
    fn key_check(&mut self) {
        if self.lang == Lang::Json && self.next_sig(self.i) == Some(':') {
            if let Some(last) = self.out.last_mut() {
                last.0 = Kind::Type;
            }
        }
    }

    fn raw_body(&mut self, hashes: usize) {
        let from = self.i;
        while self.i < self.cs.len() {
            if self.cs[self.i] == '"' && (1..=hashes).all(|k| self.cs.get(self.i + k) == Some(&'#')) {
                self.i += 1 + hashes;
                self.push(Kind::Str, from, self.i);
                self.carry = Carry::None;
                return;
            }
            self.i += 1;
        }
        self.push(Kind::Str, from, self.i);
        self.carry = Carry::RawStr { hashes };
    }

    fn step(&mut self) {
        let c = self.cs[self.i];
        if c.is_whitespace() {
            let from = self.i;
            while self.i < self.cs.len() && self.cs[self.i].is_whitespace() {
                self.i += 1;
            }
            self.push(Kind::Plain, from, self.i);
            return;
        }
        // A line comment (in a shell, a YAML or a TOML file only where a word starts).
        let hash_ok = !matches!(self.lang, Lang::Shell | Lang::Yaml | Lang::Toml) || self.i == 0 || self.cs[self.i - 1].is_whitespace();
        if hash_ok && self.spec.line.iter().any(|m| self.starts(self.i, m)) {
            let (from, to) = (self.i, self.cs.len());
            self.push(Kind::Comment, from, to);
            self.i = to;
            return;
        }
        if let Some((open, _)) = self.spec.block {
            if self.starts(self.i, open) {
                let from = self.i;
                self.i += open.chars().count();
                // The scan of the body starts after the opener; the opener belongs to the comment.
                let start = self.out.len();
                self.block_body(1);
                if self.out.len() > start {
                    self.out[start].1.insert_str(0, open);
                } else {
                    self.push(Kind::Comment, from, self.i);
                }
                return;
            }
        }
        if self.lang == Lang::Markup {
            self.markup_step(c);
            return;
        }
        if self.lang == Lang::Rust && self.rust_raw_start() {
            return;
        }
        if self.lang == Lang::Python && self.python_prefixed() {
            return;
        }
        if c == '\'' && self.lang == Lang::Rust {
            self.rust_quote();
            return;
        }
        if self.spec.quotes.contains(&c) {
            self.open_string(c);
            return;
        }
        if self.lang == Lang::Css && c == '#' && self.cs.get(self.i + 1).is_some_and(char::is_ascii_hexdigit) {
            let from = self.i;
            self.i += 1;
            while self.i < self.cs.len() && self.cs[self.i].is_alphanumeric() {
                self.i += 1;
            }
            self.push(Kind::Number, from, self.i);
            return;
        }
        if c.is_ascii_digit() && !self.prev_is_ident() {
            let from = self.i;
            while self.i < self.cs.len() && (self.cs[self.i].is_alphanumeric() || matches!(self.cs[self.i], '.' | '_') || (self.lang == Lang::Css && self.cs[self.i] == '%')) {
                self.i += 1;
            }
            self.push(Kind::Number, from, self.i);
            return;
        }
        if self.lang == Lang::Shell && c == '$' {
            self.shell_variable();
            return;
        }
        if (self.lang == Lang::Python && c == '@' && self.cs.get(self.i + 1).is_some_and(|n| self.ident_start(*n))) || (self.lang == Lang::Css && c == '@') {
            let from = self.i;
            self.i += 1;
            while self.i < self.cs.len() && (self.ident_char(self.cs[self.i]) || self.cs[self.i] == '.') {
                self.i += 1;
            }
            self.push(if self.lang == Lang::Css { Kind::Keyword } else { Kind::Meta }, from, self.i);
            return;
        }
        if self.ident_start(c) {
            self.word();
            return;
        }
        let from = self.i;
        self.i += 1;
        self.push(if c.is_ascii_punctuation() { Kind::Punct } else { Kind::Plain }, from, self.i);
    }

    /// `$name`, `${name}`, `$1`, `$@`: a variable.
    fn shell_variable(&mut self) {
        let from = self.i;
        self.i += 1;
        match self.cs.get(self.i) {
            Some('{') => {
                while self.i < self.cs.len() && self.cs[self.i] != '}' {
                    self.i += 1;
                }
                self.i = (self.i + 1).min(self.cs.len());
            }
            Some(c) if c.is_alphanumeric() || *c == '_' => {
                while self.i < self.cs.len() && (self.cs[self.i].is_alphanumeric() || self.cs[self.i] == '_') {
                    self.i += 1;
                }
            }
            Some('@' | '*' | '?' | '#' | '$') => self.i += 1,
            _ => {}
        }
        self.push(Kind::Meta, from, self.i);
    }

    fn word(&mut self) {
        let from = self.i;
        while self.i < self.cs.len() && self.ident_char(self.cs[self.i]) {
            self.i += 1;
        }
        let word = self.text(from, self.i);
        let is_kw = if self.spec.nocase { self.spec.keywords.contains(&word.to_lowercase().as_str()) } else { self.spec.keywords.contains(&word.as_str()) };
        let next = self.next_sig(self.i);
        let kind = if matches!(self.lang, Lang::Yaml | Lang::Toml) && next == Some(if self.lang == Lang::Yaml { ':' } else { '=' }) {
            Kind::Type
        } else if is_kw {
            Kind::Keyword
        } else if self.lang == Lang::Rust && self.cs.get(self.i) == Some(&'!') && self.cs.get(self.i + 1) != Some(&'=') {
            Kind::Meta
        } else if self.spec.types && word.chars().next().is_some_and(char::is_uppercase) && word.chars().any(char::is_lowercase) {
            Kind::Type
        } else {
            Kind::Plain
        };
        self.push(kind, from, self.i);
    }

    fn open_string(&mut self, quote: char) {
        let from = self.i;
        let triple = self.spec.triple && self.starts(self.i, &quote.to_string().repeat(3));
        let head = if triple { 3 } else { 1 };
        self.i += head;
        // The body is scanned from here; the opener belongs to the token.
        let start = self.out.len();
        self.string_body(quote, triple);
        if self.out.len() > start {
            let opener = self.text(from, from + head);
            self.out[start].1.insert_str(0, &opener);
        } else {
            self.push(Kind::Str, from, self.i);
        }
    }

    /// `r"…"`, `r#"…"#`, `br"…"`: a raw string, whose body ends only at a quote with as many `#` after it.
    fn rust_raw_start(&mut self) -> bool {
        if self.prev_is_ident() {
            return false;
        }
        let mut j = self.i;
        if self.cs[j] == 'b' {
            j += 1;
        }
        if self.cs.get(j) != Some(&'r') {
            return false;
        }
        j += 1;
        let hashes = self.cs[j..].iter().take_while(|c| **c == '#').count();
        if self.cs.get(j + hashes) != Some(&'"') {
            return false;
        }
        let from = self.i;
        self.i = j + hashes + 1;
        let start = self.out.len();
        self.raw_body(hashes);
        if self.out.len() > start {
            let head = self.text(from, j + hashes + 1);
            self.out[start].1.insert_str(0, &head);
        } else {
            self.push(Kind::Str, from, self.i);
        }
        true
    }

    /// A Python string prefix (`r`, `b`, `f`, `u`, two of them) right before a quote belongs to the string.
    fn python_prefixed(&mut self) -> bool {
        if self.prev_is_ident() {
            return false;
        }
        let n = self.cs[self.i..].iter().take_while(|c| "rRbBfFuU".contains(**c)).count();
        if n == 0 || n > 2 || !matches!(self.cs.get(self.i + n), Some('"' | '\'')) {
            return false;
        }
        let from = self.i;
        let quote = self.cs[self.i + n];
        self.i += n;
        let start = self.out.len();
        self.open_string(quote);
        if self.out.len() > start {
            let head = self.text(from, from + n);
            self.out[start].1.insert_str(0, &head);
        }
        true
    }

    /// A single quote in Rust is a character (`'x'`, `'\n'`, `'\u{1F600}'`) or a lifetime (`'a`).
    fn rust_quote(&mut self) {
        let from = self.i;
        let next = self.cs.get(self.i + 1).copied();
        if next == Some('\\') {
            // The character after the backslash is part of the escape (`'\\''`), so the closing quote is looked for after it.
            let mut j = self.i + 3;
            while j < self.cs.len() && self.cs[j] != '\'' {
                j += 1;
            }
            self.i = (j + 1).min(self.cs.len());
            self.push(Kind::Str, from, self.i);
        } else if next.is_some() && self.cs.get(self.i + 2) == Some(&'\'') {
            self.i += 3;
            self.push(Kind::Str, from, self.i);
        } else if next.is_some_and(|n| self.ident_start(n)) {
            self.i += 1;
            while self.i < self.cs.len() && self.ident_char(self.cs[self.i]) {
                self.i += 1;
            }
            self.push(Kind::Type, from, self.i);
        } else {
            self.i += 1;
            self.push(Kind::Punct, from, self.i);
        }
    }

    /// HTML and XML: tag names are keywords, attribute names types, attribute values strings; text outside a tag stays plain.
    fn markup_step(&mut self, c: char) {
        let from = self.i;
        if c == '<' && self.cs.get(self.i + 1).is_some_and(|n| n.is_alphabetic() || matches!(n, '/' | '!' | '?')) {
            self.i += 1;
            if matches!(self.cs.get(self.i), Some('/' | '!' | '?')) {
                self.i += 1;
            }
            self.push(Kind::Punct, from, self.i);
            let name = self.i;
            while self.i < self.cs.len() && (self.cs[self.i].is_alphanumeric() || matches!(self.cs[self.i], '-' | ':' | '.' | '_')) {
                self.i += 1;
            }
            self.push(Kind::Keyword, name, self.i);
            self.in_tag = true;
            return;
        }
        if !self.in_tag {
            self.i += 1;
            self.push(Kind::Plain, from, self.i);
            return;
        }
        if c == '>' {
            self.in_tag = false;
            self.i += 1;
            self.push(Kind::Punct, from, self.i);
        } else if c == '"' || c == '\'' {
            self.open_string(c);
        } else if self.ident_start(c) {
            while self.i < self.cs.len() && (self.cs[self.i].is_alphanumeric() || matches!(self.cs[self.i], '-' | ':' | '.' | '_')) {
                self.i += 1;
            }
            self.push(Kind::Type, from, self.i);
        } else {
            self.i += 1;
            self.push(if c.is_ascii_punctuation() { Kind::Punct } else { Kind::Plain }, from, self.i);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(lang: Lang, text: &str) -> Vec<Vec<(Kind, String)>> {
        let (lines, _) = highlight(lang, text).unwrap();
        lines.into_iter().map(|l| l.toks.into_iter().map(|t| (t.kind, t.text)).collect()).collect()
    }

    fn has(line: &[(Kind, String)], kind: Kind, text: &str) -> bool {
        line.iter().any(|(k, t)| *k == kind && t.trim() == text)
    }

    #[test]
    fn a_name_gets_the_view_its_extension_says() {
        assert_eq!(view_of("README.md"), View::Markdown);
        assert_eq!(view_of("src\\Main.JAVA"), View::Code(Lang::Java));
        assert_eq!(view_of("a/b/tool.py"), View::Code(Lang::Python));
        assert_eq!(view_of("lib.rs"), View::Code(Lang::Rust));
        assert_eq!(view_of("Makefile"), View::Plain);
        assert_eq!(view_of("data.xyz"), View::Plain);
        assert_eq!(view_of(".md"), View::Plain, "a dot file is a name, not an extension");
        assert_eq!(view_of("dir.md/readme"), View::Plain);
    }

    #[test]
    fn a_python_triple_quoted_string_runs_over_lines_and_a_hash_in_a_string_is_no_comment() {
        let got = kinds(Lang::Python, "x = \"\"\"a\nb\"\"\"\ny = 1  # n\ns = '#'\nt = ''\nu = 2");
        assert_eq!(got[0].last().unwrap().0, Kind::Str);
        assert!(got[1].iter().all(|(k, _)| *k == Kind::Str), "{:?}", got[1]);
        assert!(has(&got[2], Kind::Number, "1") && has(&got[2], Kind::Comment, "# n"), "{:?}", got[2]);
        assert!(has(&got[3], Kind::Str, "'#'"));
        assert!(has(&got[4], Kind::Str, "''") && has(&got[5], Kind::Number, "2"), "an empty string is not a triple quote");
    }

    #[test]
    fn a_java_block_comment_runs_over_lines() {
        let got = kinds(Lang::Java, "int a; /* x\n y */ int b;");
        assert!(has(&got[0], Kind::Keyword, "int") && got[0].last().unwrap().0 == Kind::Comment);
        assert_eq!(got[1][0].0, Kind::Comment);
        assert_eq!(got[1][0].1, " y */");
        assert!(has(&got[1], Kind::Keyword, "int"), "{:?}", got[1]);
    }

    #[test]
    fn an_escaped_quote_does_not_end_a_string() {
        let got = kinds(Lang::Java, "s = \"a\\\"b\" + c;");
        assert!(got[0].iter().any(|(k, t)| *k == Kind::Str && t.contains("a\\\"b")), "{:?}", got[0]);
        let got = kinds(Lang::Java, "s = \"a\\\\\" x;");
        assert!(has(&got[0], Kind::Str, "\"a\\\\\""), "{:?}", got[0]);
        assert!(!got[0].iter().any(|(k, t)| *k == Kind::Str && t.contains(" x")));
    }

    #[test]
    fn rust_lifetimes_characters_raw_strings_and_nested_comments() {
        let got = kinds(Lang::Rust, "fn f<'a>(x: &'a str) -> char { 'x' }");
        assert!(got[0].iter().all(|(k, t)| !(*k == Kind::Str && t.contains("'a"))), "{:?}", got[0]);
        assert!(has(&got[0], Kind::Str, "'x'"));
        for ch in ["'\\n'", "'\\''", "'\\u{1F600}'"] {
            let got = kinds(Lang::Rust, &format!("let c = {ch};"));
            assert!(has(&got[0], Kind::Str, ch), "{ch}: {:?}", got[0]);
        }
        let got = kinds(Lang::Rust, "let s = r#\"a\"b\"#; let t = 1;");
        assert!(has(&got[0], Kind::Str, "r#\"a\"b\"#"), "{:?}", got[0]);
        assert!(has(&got[0], Kind::Number, "1"));
        let got = kinds(Lang::Rust, "/* /* */ still */ fn");
        assert_eq!(got[0][0], (Kind::Comment, "/* /* */ still */".into()));
        assert!(has(&got[0], Kind::Keyword, "fn"));
        let got = kinds(Lang::Rust, "println!(\"a\\nb\");");
        assert!(has(&got[0], Kind::Meta, "println"), "{:?}", got[0]);
    }

    #[test]
    fn comment_markers_inside_strings_and_words_are_left_alone() {
        let got = kinds(Lang::JsTs, "const u = \"http://x\"; // done\nlet t = `a\n// b`;\n");
        assert!(has(&got[0], Kind::Str, "\"http://x\"") && has(&got[0], Kind::Comment, "// done"), "{:?}", got[0]);
        assert!(got[1].last().unwrap().0 == Kind::Str && got[2][0].0 == Kind::Str, "a template string runs over lines");
        let got = kinds(Lang::Shell, "echo a#b\n# c\nx=$HOME ${y}");
        assert!(got[0].iter().all(|(k, _)| *k != Kind::Comment), "{:?}", got[0]);
        assert_eq!(got[1][0].0, Kind::Comment);
        assert_eq!(got[2].last().unwrap(), &(Kind::Meta, "$HOME ${y}".to_owned()), "{:?}", got[2]);
    }

    #[test]
    fn a_single_quoted_string_without_its_end_ends_with_the_line() {
        let got = kinds(Lang::CLike, "char c = 'a;\nint x = 1;");
        assert!(has(&got[1], Kind::Keyword, "int") && has(&got[1], Kind::Number, "1"), "{:?}", got[1]);
    }

    #[test]
    fn json_keys_toml_keys_and_markup_tags_have_their_own_colours() {
        let got = kinds(Lang::Json, "{\"a\": [1, true], \"b\": \"c\"}");
        assert!(has(&got[0], Kind::Type, "\"a\"") && has(&got[0], Kind::Type, "\"b\""), "{:?}", got[0]);
        assert!(has(&got[0], Kind::Str, "\"c\"") && has(&got[0], Kind::Keyword, "true"), "{:?}", got[0]);
        let got = kinds(Lang::Toml, "[package]\nname = \"x\" # c");
        assert_eq!(got[0][0].0, Kind::Meta);
        assert!(has(&got[1], Kind::Type, "name") && has(&got[1], Kind::Str, "\"x\"") && has(&got[1], Kind::Comment, "# c"), "{:?}", got[1]);
        let got = kinds(Lang::Markup, "<a href=\"x\">hi it's</a><!-- c\nd -->");
        assert!(has(&got[0], Kind::Keyword, "a") && has(&got[0], Kind::Type, "href") && has(&got[0], Kind::Str, "\"x\""), "{:?}", got[0]);
        assert!(got[0].iter().all(|(k, t)| !(*k == Kind::Str && t.contains("it's"))), "text outside a tag is not a string");
        assert_eq!(got[1][0], (Kind::Comment, "d -->".into()));
    }

    #[test]
    fn the_tokens_of_a_line_are_the_line_and_a_blank_run_is_never_a_token() {
        let text = "def f(x):\n    return \"한글\"  # 주석   \n\n\t\tpass\n  \n";
        let (lines, _) = highlight(Lang::Python, text).unwrap();
        for (line, shown) in lines.iter().zip(display_lines(text)) {
            let joined: String = line.toks.iter().map(|t| t.text.as_str()).collect();
            assert_eq!(joined, shown.trim_end(), "line {}", line.n);
            assert!(line.toks.iter().all(|t| !t.text.trim().is_empty()));
            assert!(line.toks.windows(2).all(|w| w[0].kind != w[1].kind), "neighbours of one kind are joined: {:?}", line.toks);
        }
        assert!(lines[2].toks.is_empty() && lines[4].toks.is_empty());
        assert_eq!(lines[1].toks[0].text, "    return", "the indent starts the token it is drawn with");
    }

    #[test]
    fn tabs_a_byte_order_mark_and_line_ends_are_made_for_drawing() {
        assert_eq!(display_lines("\tx"), ["    x"]);
        assert_eq!(display_lines("ab\tc"), ["ab  c"]);
        assert_eq!(display_lines("\u{feff}a\r\nb"), ["a", "b"]);
        assert_eq!(display_lines("한\tx"), ["한  x"], "a wide character takes two columns");
        assert_eq!(char_cols('가'), 2);
        assert_eq!(char_cols('a'), 1);
    }

    #[test]
    fn a_text_too_long_gets_no_colours_and_a_line_too_long_is_one_plain_token() {
        let long = "x\n".repeat(MAX_GUTTER + 1);
        assert!(highlight(Lang::Rust, &long).is_none());
        let wide = "a".repeat(MAX_LINE_CHARS + 1);
        let (lines, widest) = highlight(Lang::Rust, &format!("{wide} // c\nlet x = 1;")).unwrap();
        assert_eq!(lines[0].toks.len(), 1);
        assert!(lines[0].toks[0].text.starts_with(&wide) && lines[0].toks[0].kind == Kind::Plain);
        assert_eq!(widest, wide.len() + 5);
        assert!(lines[1].toks.iter().any(|t| t.kind == Kind::Keyword));
    }

    #[test]
    fn text_in_other_scripts_inside_strings_and_comments_does_not_panic() {
        for lang in [Lang::Rust, Lang::Java, Lang::Python, Lang::JsTs, Lang::Markup, Lang::Shell, Lang::Json, Lang::Yaml, Lang::Css, Lang::Sql, Lang::Slint, Lang::Go, Lang::Kotlin, Lang::CSharp, Lang::CLike, Lang::Toml] {
            let _ = highlight(lang, "\"한글\" // 주석 'é' `x` <é> r#\"a\n\"# /* ü\n */ \\\n\"unterminated\n'\n");
        }
    }
}
