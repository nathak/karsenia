//! The terminal drawer's text: the shell's output frames (see `server::term_ops`) turned into plain lines. The shell is started with
//! TERM=dumb, so what comes is text with the odd carriage return, backspace or colour code; anything that would move the cursor
//! about or set a title is dropped, never interpreted. What the shell prints is shown and nowhere else: it is not logged.
use std::collections::VecDeque;

/// One frame of the terminal channel: `[1][tid u32 BE][bytes]` is output, `[2][tid u32 BE][exit code i32 BE]` is the end.
#[derive(Debug, PartialEq)]
pub enum Frame<'a> {
    Output(u32, &'a [u8]),
    Exit(u32, i32),
}

pub fn parse_frame(bytes: &[u8]) -> Option<Frame<'_>> {
    let (&kind, rest) = bytes.split_first()?;
    let tid = u32::from_be_bytes(rest.get(..4)?.try_into().ok()?);
    let body = &rest[4..];
    match kind {
        1 => Some(Frame::Output(tid, body)),
        2 => Some(Frame::Exit(tid, i32::from_be_bytes(body.try_into().ok()?))),
        _ => None,
    }
}

const MAX_LINES: usize = 2000;
const MAX_LINE_CHARS: usize = 2000;
const MAX_TOTAL_CHARS: usize = 128 * 1024;

#[derive(Clone, Copy, PartialEq)]
enum Esc {
    None,
    /// After ESC.
    Start,
    /// ESC [ ... until a final byte.
    Csi,
    /// ESC ] ... until BEL or ESC \.
    Osc,
    OscEnd,
    /// ESC ( x: one more character to skip.
    Skip,
}

pub struct Scrollback {
    lines: VecDeque<String>,
    cur: Vec<char>,
    cursor: usize,
    total: usize,
    esc: Esc,
    /// The start of a UTF-8 character that the last chunk cut off.
    partial: Vec<u8>,
}

impl Default for Scrollback {
    fn default() -> Self {
        Scrollback { lines: VecDeque::new(), cur: Vec::new(), cursor: 0, total: 0, esc: Esc::None, partial: Vec::new() }
    }
}

impl Scrollback {
    pub fn clear(&mut self) {
        *self = Scrollback::default();
    }

    pub fn push(&mut self, chunk: &[u8]) {
        let mut bytes = std::mem::take(&mut self.partial);
        bytes.extend_from_slice(chunk);
        let valid = match std::str::from_utf8(&bytes) {
            Ok(_) => bytes.len(),
            Err(e) if e.error_len().is_none() => e.valid_up_to(),
            // Bytes that are not UTF-8 at all become the replacement character, one by one.
            Err(_) => {
                let text = String::from_utf8_lossy(&bytes).into_owned();
                for c in text.chars() {
                    self.char(c);
                }
                return;
            }
        };
        let (done, rest) = bytes.split_at(valid);
        // A cut-off character is kept for the next chunk, but never more than the longest one.
        self.partial = rest.iter().copied().take(3).collect();
        for c in String::from_utf8_lossy(done).chars() {
            self.char(c);
        }
    }

    fn char(&mut self, c: char) {
        match self.esc {
            Esc::Start => {
                self.esc = match c {
                    '[' => Esc::Csi,
                    ']' => Esc::Osc,
                    '(' | ')' | '#' | '%' => Esc::Skip,
                    _ => Esc::None,
                };
                return;
            }
            Esc::Csi => {
                if ('\u{40}'..='\u{7e}').contains(&c) {
                    self.esc = Esc::None;
                }
                return;
            }
            Esc::Osc => {
                match c {
                    '\u{7}' => self.esc = Esc::None,
                    '\u{1b}' => self.esc = Esc::OscEnd,
                    _ => {}
                }
                return;
            }
            Esc::OscEnd => {
                self.esc = if c == '\\' { Esc::None } else { Esc::Osc };
                return;
            }
            Esc::Skip => {
                self.esc = Esc::None;
                return;
            }
            Esc::None => {}
        }
        match c {
            '\u{1b}' => self.esc = Esc::Start,
            '\n' => self.newline(),
            '\r' => self.cursor = 0,
            '\u{8}' => self.cursor = self.cursor.saturating_sub(1),
            '\t' => {
                let to = (self.cursor / 8 + 1) * 8;
                // A full line takes no more: the loop ends when a space cannot be added.
                while self.cursor < to && self.put(' ') {}
            }
            c if c.is_control() => {}
            c => {
                self.put(c);
            }
        }
    }

    /// Puts one character at the cursor; false when the line is full and nothing was put.
    fn put(&mut self, c: char) -> bool {
        if self.cursor < self.cur.len() {
            self.cur[self.cursor] = c;
        } else if self.cur.len() < MAX_LINE_CHARS {
            self.cur.push(c);
            self.total += 1;
        } else {
            return false;
        }
        self.cursor += 1;
        true
    }

    fn newline(&mut self) {
        let line: String = self.cur.drain(..).collect();
        self.cursor = 0;
        self.lines.push_back(line);
        self.total += 1;
        while self.lines.len() > MAX_LINES || self.total > MAX_TOTAL_CHARS {
            let Some(old) = self.lines.pop_front() else { break };
            self.total = self.total.saturating_sub(old.chars().count() + 1);
        }
    }

    /// Everything shown so far, the unfinished line (the prompt) last.
    pub fn text(&self) -> String {
        let mut out = String::with_capacity(self.total + 16);
        for line in &self.lines {
            out.push_str(line);
            out.push('\n');
        }
        out.extend(self.cur.iter());
        out
    }
}

/// A frame that came before the answer to `term_open` said which shell it belongs to.
enum Stashed {
    Out(Vec<u8>),
    Exit(i32),
}

const STASH_BYTES: usize = 64 * 1024;
const STASH_ITEMS: usize = 64;

/// The drawer's one shell: which open request it waits for, which shell is open, and what it printed. A shell is accepted only for
/// the request that asked for it, so an answer that comes late (the drawer was closed, the chat changed, another open was made
/// since) never becomes the shell of the chat now on screen; it is handed back to be closed.
#[derive(Default)]
pub struct TermState {
    pub sb: Scrollback,
    tid: Option<u32>,
    /// The `term_open` that is on its way.
    want: Option<u64>,
    /// Shells closed or ended: whatever of theirs is still in flight is dropped.
    dead: std::collections::BTreeSet<u32>,
    stash: Vec<(u32, Stashed)>,
    stash_bytes: usize,
    /// A line for under the output ("" for none).
    pub status: String,
    /// A command typed (in the composer's shell mode) before the shell was ready: it is sent once the shell is accepted.
    queued: Option<String>,
}

impl TermState {
    pub fn tid(&self) -> Option<u32> {
        self.tid
    }

    /// Keeps a command for the shell that is being opened (the newest one only).
    pub fn queue(&mut self, line: String) {
        if self.want.is_some() {
            self.queued = Some(line);
        }
    }

    /// The command that waited for the shell, once, after the shell was accepted.
    pub fn take_queued(&mut self) -> Option<String> {
        if self.tid.is_some() {
            return self.queued.take();
        }
        // No shell, and none being opened: the command is not kept for a later one.
        if self.want.is_none() {
            self.queued = None;
        }
        None
    }

    pub fn busy(&self) -> bool {
        self.tid.is_some() || self.want.is_some()
    }

    /// An open was sent as request `req`: output starts afresh.
    pub fn begin(&mut self, req: u64) {
        self.sb.clear();
        self.status.clear();
        self.stash.clear();
        self.stash_bytes = 0;
        self.queued = None;
        self.want = Some(req);
    }

    /// The drawer was closed or the chat changed: the shell to close (if one is open), and everything forgotten.
    pub fn close(&mut self) -> Option<u32> {
        let tid = self.tid.take();
        if let Some(tid) = tid {
            self.mark_dead(tid);
        }
        self.want = None;
        self.queued = None;
        self.sb.clear();
        self.status.clear();
        self.stash.clear();
        self.stash_bytes = 0;
        tid
    }

    fn mark_dead(&mut self, tid: u32) {
        self.dead.insert(tid);
        while self.dead.len() > 256 {
            if let Some(first) = self.dead.iter().next().copied() {
                self.dead.remove(&first);
            }
        }
    }

    /// The answer to an open. True: this is the shell that was asked for. False: it is not (the caller closes it).
    pub fn opened(&mut self, req: Option<u64>, tid: u32, ended: &dyn Fn(i32) -> String) -> bool {
        let wanted = self.want.is_some() && self.want == req && self.tid.is_none() && !self.dead.contains(&tid);
        if !wanted {
            self.mark_dead(tid);
            self.stash.retain(|(t, _)| *t != tid);
            return false;
        }
        self.want = None;
        self.tid = Some(tid);
        let held = std::mem::take(&mut self.stash);
        self.stash_bytes = 0;
        for (t, item) in held {
            if t == tid {
                match item {
                    Stashed::Out(bytes) => self.sb.push(&bytes),
                    Stashed::Exit(code) => {
                        self.ended(code);
                        self.status = ended(code);
                    }
                }
            }
        }
        true
    }

    /// The open `req` was refused: nothing waits any more. An error that answers something else is not this one's.
    pub fn failed(&mut self, req: Option<u64>, message: String) {
        if self.want.is_none() || self.want != req {
            return;
        }
        self.want = None;
        self.queued = None;
        self.stash.clear();
        self.stash_bytes = 0;
        self.status = message;
    }

    fn ended(&mut self, code: i32) {
        self.queued = None;
        if let Some(tid) = self.tid.take() {
            self.mark_dead(tid);
        }
        self.status = format!("exit:{code}");
    }

    /// One frame of the terminal channel; true when what is shown changed. `ended` words the end of the shell from its code.
    pub fn frame(&mut self, frame: &Frame<'_>, ended: &dyn Fn(i32) -> String) -> bool {
        let (tid, out, code) = match *frame {
            Frame::Output(tid, bytes) => (tid, Some(bytes), 0),
            Frame::Exit(tid, code) => (tid, None, code),
        };
        if self.dead.contains(&tid) {
            return false;
        }
        if self.tid == Some(tid) {
            match out {
                Some(bytes) => self.sb.push(bytes),
                None => {
                    self.ended(code);
                    self.status = ended(code);
                }
            }
            return true;
        }
        // A shell not yet known may be the one being opened: its first output waits (a little) for the answer.
        if self.want.is_some() && self.tid.is_none() {
            match out {
                Some(bytes) if self.stash_bytes + bytes.len() <= STASH_BYTES && self.stash.len() < STASH_ITEMS => {
                    self.stash_bytes += bytes.len();
                    self.stash.push((tid, Stashed::Out(bytes.to_vec())));
                }
                None if self.stash.len() < STASH_ITEMS => self.stash.push((tid, Stashed::Exit(code))),
                _ => {}
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn show(chunks: &[&[u8]]) -> String {
        let mut s = Scrollback::default();
        for c in chunks {
            s.push(c);
        }
        s.text()
    }

    #[test]
    fn frames_are_read_by_their_kind_and_a_bad_one_is_refused() {
        let mut out = vec![1u8, 0, 0, 0, 7];
        out.extend_from_slice(b"hi");
        assert_eq!(parse_frame(&out), Some(Frame::Output(7, b"hi")));
        let mut exit = vec![2u8, 0, 0, 0, 9];
        exit.extend_from_slice(&(-1i32).to_be_bytes());
        assert_eq!(parse_frame(&exit), Some(Frame::Exit(9, -1)));
        assert_eq!(parse_frame(&[2, 0, 0, 0, 9, 1]), None, "an exit code is four bytes");
        assert_eq!(parse_frame(&[1, 0, 0]), None);
        assert_eq!(parse_frame(&[3, 0, 0, 0, 1]), None);
        assert_eq!(parse_frame(&[]), None);
    }

    #[test]
    fn lines_carriage_returns_and_backspaces_are_worked_out() {
        assert_eq!(show(&[b"a\r\nb\r\n$ "]), "a\nb\n$ ");
        assert_eq!(show(&[b"abc\rX"]), "Xbc");
        assert_eq!(show(&[b"abc\x08\x08Z"]), "aZc");
        assert_eq!(show(&[b"a\tb"]), "a       b");
        assert_eq!(show(&[b"bell\x07 ok\x00"]), "bell ok");
    }

    #[test]
    fn colour_codes_titles_and_other_escapes_are_dropped_even_when_split() {
        assert_eq!(show(&[b"\x1b[31mred\x1b[0m"]), "red");
        assert_eq!(show(&[b"\x1b[3", b"1mred"]), "red");
        assert_eq!(show(&[b"\x1b]0;title\x07ok"]), "ok");
        assert_eq!(show(&[b"\x1b]0;title\x1b\\ok"]), "ok");
        assert_eq!(show(&[b"\x1b]0;ti", b"tle\x07ok"]), "ok");
        assert_eq!(show(&[b"\x1b(Bok\x1bcgo"]), "okgo");
        assert_eq!(show(&[b"\x1b[2J\x1b[Hhome"]), "home");
    }

    #[test]
    fn a_character_cut_between_chunks_is_kept_whole_and_bad_bytes_are_replaced() {
        let bytes = "가나".as_bytes();
        assert_eq!(show(&[&bytes[..1], &bytes[1..4], &bytes[4..]]), "가나");
        assert_eq!(show(&[&[0xff, b'a']]), "\u{fffd}a");
    }

    #[test]
    fn the_scrollback_is_kept_to_its_limits() {
        let mut s = Scrollback::default();
        for i in 0..(MAX_LINES + 50) {
            s.push(format!("line {i}\n").as_bytes());
        }
        let text = s.text();
        assert!(text.lines().count() <= MAX_LINES + 1);
        assert!(text.starts_with("line 50\n"), "the oldest lines go first");
        let mut wide = Scrollback::default();
        wide.push("x".repeat(MAX_LINE_CHARS * 3).as_bytes());
        assert_eq!(wide.text().chars().count(), MAX_LINE_CHARS);
        let mut big = Scrollback::default();
        for _ in 0..200 {
            big.push(format!("{}\n", "y".repeat(1500)).as_bytes());
        }
        assert!(big.text().chars().count() <= MAX_TOTAL_CHARS + MAX_LINE_CHARS);
        big.clear();
        assert_eq!(big.text(), "");
    }

    #[test]
    fn a_tab_after_a_full_line_ends() {
        let mut s = Scrollback::default();
        s.push("x".repeat(MAX_LINE_CHARS).as_bytes());
        s.push(b"\tmore\tand\tmore");
        assert_eq!(s.text().chars().count(), MAX_LINE_CHARS);
    }

    fn word(code: i32) -> String {
        format!("ended {code}")
    }

    #[test]
    fn output_before_the_answer_waits_and_then_shows() {
        let mut t = TermState::default();
        t.begin(7);
        assert!(!t.frame(&Frame::Output(3, b"$ "), &word), "not shown before it is known");
        assert!(t.opened(Some(7), 3, &word));
        assert_eq!(t.sb.text(), "$ ");
        assert!(t.frame(&Frame::Output(3, b"ls\n"), &word));
        assert_eq!(t.tid(), Some(3));
        assert!(t.frame(&Frame::Exit(3, 2), &word));
        assert_eq!(t.status, "ended 2");
        assert_eq!(t.tid(), None);
        assert!(!t.frame(&Frame::Output(3, b"late"), &word), "nothing of an ended shell");
    }

    #[test]
    fn a_shell_that_ended_before_its_answer_says_so_in_words() {
        let mut t = TermState::default();
        t.begin(1);
        t.frame(&Frame::Output(4, b"bye\n"), &word);
        t.frame(&Frame::Exit(4, 3), &word);
        assert!(t.opened(Some(1), 4, &word));
        assert_eq!(t.status, "ended 3");
        assert_eq!(t.sb.text(), "bye\n");
        assert_eq!(t.tid(), None);
    }

    #[test]
    fn a_late_answer_never_becomes_the_shell_of_a_later_open() {
        let mut t = TermState::default();
        // Chat A: open sent as request 1, drawer closed before the answer; chat B: open sent as request 2.
        t.begin(1);
        assert_eq!(t.close(), None, "no shell yet, so nothing to close");
        t.begin(2);
        t.frame(&Frame::Output(10, b"A-prompt"), &word);
        t.frame(&Frame::Output(11, b"B-prompt"), &word);
        // A's answer arrives first: refused, so the caller closes it.
        assert!(!t.opened(Some(1), 10, &word));
        assert!(t.opened(Some(2), 11, &word));
        assert_eq!(t.sb.text(), "B-prompt", "A's output is dropped, B's kept");
        assert!(!t.frame(&Frame::Output(10, b"more of A"), &word));
        assert_eq!(t.tid(), Some(11));
        // An answer that names no request, or a second answer, is refused too.
        assert!(!t.opened(None, 12, &word));
        assert!(!t.opened(Some(2), 13, &word));
    }

    #[test]
    fn closing_forgets_the_shell_and_what_it_printed_and_a_failure_ends_the_wait() {
        let mut t = TermState::default();
        t.begin(1);
        assert!(t.opened(Some(1), 5, &word));
        t.frame(&Frame::Output(5, b"secret-ish"), &word);
        assert_eq!(t.close(), Some(5));
        assert_eq!(t.sb.text(), "");
        assert!(!t.frame(&Frame::Output(5, b"after close"), &word));
        assert!(!t.busy());
        t.begin(2);
        assert!(t.busy());
        t.failed(Some(1), "an older open failed".into());
        assert!(t.busy(), "another request's error is not this open's");
        t.failed(Some(2), "the terminal is off".into());
        assert!(!t.busy());
        assert_eq!(t.status, "the terminal is off");
        assert!(!t.opened(Some(2), 6, &word), "a shell nobody waits for is refused");
    }

    #[test]
    fn what_waits_for_an_answer_is_limited() {
        let mut t = TermState::default();
        t.begin(1);
        for i in 0..200u32 {
            t.frame(&Frame::Output(100 + i, &[b'x'; 2000]), &word);
        }
        assert!(t.stash_bytes <= STASH_BYTES && t.stash.len() <= STASH_ITEMS);
    }

    #[test]
    fn a_command_typed_before_the_shell_is_ready_is_sent_once_when_it_is() {
        let mut t = TermState::default();
        t.queue("ls".into());
        assert_eq!(t.take_queued(), None, "nothing is being opened: nothing is kept");
        t.begin(1);
        t.queue("ls".into());
        t.queue("pwd".into());
        assert_eq!(t.take_queued(), None, "not before the shell is accepted");
        assert!(t.opened(Some(1), 4, &word));
        assert_eq!(t.take_queued().as_deref(), Some("pwd"));
        assert_eq!(t.take_queued(), None, "only once");
        t.begin(2);
        t.queue("x".into());
        t.failed(Some(2), "off".into());
        assert_eq!(t.take_queued(), None, "a refused open drops it");
    }

    #[test]
    fn a_command_whose_shell_ended_first_is_not_kept_for_the_next_one() {
        let mut t = TermState::default();
        t.begin(1);
        t.queue("echo queued".into());
        t.frame(&Frame::Exit(4, 1), &word);
        assert!(t.opened(Some(1), 4, &word));
        assert_eq!(t.take_queued(), None);
        // The terminal is opened again with nothing typed: nothing is sent to it.
        t.begin(2);
        assert!(t.opened(Some(2), 5, &word));
        assert_eq!(t.take_queued(), None);
    }
}
