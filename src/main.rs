// claude-statusline
//
// A fast statusline for Claude Code: reads the statusline JSON on stdin and
// prints one ANSI-coloured line.
//
// Written with std only (no serde_json): a tiny JSON parser sufficient for
// Claude Code's shape, so the build has no dependencies.
//
// Output format:
//   <profile> <dir> [<branch>] | c:<K>k:USED%/REMAIN% [| s:USED%/REMAIN% [<reset>] | <tier>] [| <model> [<effort>]] [| idle <T>]
//   <K>k    = tokens currently in the context window (input+output), rounded to nearest 1k
//   <reset> = time until the 5-hour rate-limit window resets, e.g. 2h13m / 45m
//   <tier>  = subscription tier, shown only when rate_limits is absent
//   <T>     = time since the transcript last changed (a prompt from you or
//             output from Claude), in IDLE_STEP_MIN steps; hidden until the
//             first step. Set statusLine.refreshInterval (e.g. 60) in
//             settings.json so it advances while nothing else is happening.
//
// Palettes:
//   context (c): green / orange / red   at <50 / <80 / >=80 (red from 180k tokens)
//   session (s): bright blue / bright magenta / bright red
//   branch: yellow; model + effort: dim; idle: pastel orange
//
// Security: every string printed comes from outside this program (the JSON,
// the environment, .git/HEAD of whatever directory you're in, the credentials
// file). Control characters are stripped from all of them so nothing can
// inject terminal escape sequences, and the branch is limited to git-ref-safe
// ASCII.

// Startup cost dominates (the render itself is ~10µs), so skip std's runtime
// init: `fn main` would first read /proc/self/maps, install SIGSEGV/SIGBUS
// handlers on an alt stack, poll fds 0-2 and ignore SIGPIPE. None of that is
// needed here; a closed stdout just ends the process.
#![cfg_attr(not(test), no_main)]
// run() and the real fs readers are only reached from the no_mangle main.
#![cfg_attr(test, allow(dead_code))]

use std::borrow::Cow;
use std::fs;
use std::io::{self, Read, Write};
use std::time::{SystemTime, UNIX_EPOCH};

// Force the context (c:) segment red once it reaches this many thousand tokens,
// regardless of percentage. Warns before the 200k mark where per-token cost jumps.
const CTX_TOKEN_WARN_K: i64 = 180;

// Longest branch name shown; detached HEADs show a 7-char hash.
const MAX_BRANCH: usize = 64;
const SHORT_HASH: usize = 7;
// Cap for any other printed string (dir, model, profile, ...).
const MAX_TEXT: usize = 128;

// Idle segment granularity: hidden for the first step, then rounds down to a
// multiple of this, so it changes at most once per step.
const IDLE_STEP_MIN: i64 = 5;

// ---------- tiny JSON ----------

#[allow(dead_code)]
// Strings borrow from the input unless they contain escapes (rare), so a
// parse allocates little more than the object/array Vecs.
enum Val<'a> {
    Null,
    Bool(bool),
    Num(f64),
    Str(Cow<'a, str>),
    Arr(Vec<Val<'a>>),
    Obj(Vec<(Cow<'a, str>, Val<'a>)>),
}

impl<'a> Val<'a> {
    fn get(&self, k: &str) -> Option<&Val<'a>> {
        match self {
            Val::Obj(o) => o.iter().find(|(n, _)| n == k).map(|(_, v)| v),
            _ => None,
        }
    }
    fn as_f64(&self) -> Option<f64> {
        match self {
            Val::Num(n) => Some(*n),
            _ => None,
        }
    }
    fn as_str(&self) -> Option<&str> {
        match self {
            Val::Str(s) => Some(s),
            _ => None,
        }
    }
}

struct P<'a> {
    s: &'a str,
    b: &'a [u8],
    i: usize,
}

impl<'a> P<'a> {
    fn ws(&mut self) {
        while self.i < self.b.len() && matches!(self.b[self.i], b' ' | b'\t' | b'\n' | b'\r') {
            self.i += 1;
        }
    }
    fn peek(&self) -> Option<u8> {
        self.b.get(self.i).copied()
    }
    fn eat(&mut self) -> Option<u8> {
        let c = self.peek()?;
        self.i += 1;
        Some(c)
    }
    fn expect(&mut self, c: u8) -> Option<()> {
        if self.eat()? == c {
            Some(())
        } else {
            None
        }
    }
    fn lit<const N: usize>(&mut self, s: &[u8; N]) -> Option<()> {
        if self.b.get(self.i..self.i + N)? == &s[..] {
            self.i += N;
            Some(())
        } else {
            None
        }
    }

    fn value(&mut self) -> Option<Val<'a>> {
        self.ws();
        match self.peek()? {
            b'"' => Some(Val::Str(self.string()?)),
            b'{' => self.object(),
            b'[' => self.array(),
            b't' => {
                self.lit(b"true")?;
                Some(Val::Bool(true))
            }
            b'f' => {
                self.lit(b"false")?;
                Some(Val::Bool(false))
            }
            b'n' => {
                self.lit(b"null")?;
                Some(Val::Null)
            }
            _ => self.number(),
        }
    }

    fn string(&mut self) -> Option<Cow<'a, str>> {
        self.expect(b'"')?;
        let start = self.i;
        // Fast path: no escapes, so borrow. Both ends are at an ASCII '"',
        // which is always a char boundary, so the slice can't panic.
        while let Some(&c) = self.b.get(self.i) {
            match c {
                b'"' => {
                    let s = &self.s[start..self.i];
                    self.i += 1;
                    return Some(Cow::Borrowed(s));
                }
                b'\\' => break,
                _ => self.i += 1,
            }
        }
        // Slow path: unescape into an owned copy (or hit EOF and fail).
        let mut raw = self.b[start..self.i].to_vec();
        loop {
            let c = self.eat()?;
            match c {
                b'"' => return String::from_utf8(raw).ok().map(Cow::Owned),
                b'\\' => {
                    let e = self.eat()?;
                    match e {
                        b'"' => raw.push(b'"'),
                        b'\\' => raw.push(b'\\'),
                        b'/' => raw.push(b'/'),
                        b'n' => raw.push(b'\n'),
                        b't' => raw.push(b'\t'),
                        b'r' => raw.push(b'\r'),
                        b'b' => raw.push(0x08),
                        b'f' => raw.push(0x0C),
                        b'u' => {
                            let hex =
                                std::str::from_utf8(self.b.get(self.i..self.i + 4)?).ok()?;
                            self.i += 4;
                            let cp = u32::from_str_radix(hex, 16).ok()?;
                            if let Some(ch) = char::from_u32(cp) {
                                let mut buf = [0u8; 4];
                                raw.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                            }
                        }
                        _ => return None,
                    }
                }
                _ => raw.push(c),
            }
        }
    }

    fn number(&mut self) -> Option<Val<'a>> {
        let start = self.i;
        if self.peek() == Some(b'-') {
            self.i += 1;
        }
        while let Some(c) = self.peek() {
            if c.is_ascii_digit() || matches!(c, b'.' | b'e' | b'E' | b'+' | b'-') {
                self.i += 1;
            } else {
                break;
            }
        }
        // Only ASCII was consumed, so both ends are char boundaries.
        self.s[start..self.i].parse::<f64>().ok().map(Val::Num)
    }

    fn object(&mut self) -> Option<Val<'a>> {
        self.expect(b'{')?;
        // Most objects in both inputs have a handful of keys.
        let mut o = Vec::with_capacity(8);
        self.ws();
        if self.peek() == Some(b'}') {
            self.i += 1;
            return Some(Val::Obj(o));
        }
        loop {
            self.ws();
            let k = self.string()?;
            self.ws();
            self.expect(b':')?;
            let v = self.value()?;
            o.push((k, v));
            self.ws();
            match self.eat()? {
                b',' => continue,
                b'}' => return Some(Val::Obj(o)),
                _ => return None,
            }
        }
    }

    fn array(&mut self) -> Option<Val<'a>> {
        self.expect(b'[')?;
        let mut a = Vec::new();
        self.ws();
        if self.peek() == Some(b']') {
            self.i += 1;
            return Some(Val::Arr(a));
        }
        loop {
            let v = self.value()?;
            a.push(v);
            self.ws();
            match self.eat()? {
                b',' => continue,
                b']' => return Some(Val::Arr(a)),
                _ => return None,
            }
        }
    }
}

fn parse(s: &str) -> Option<Val<'_>> {
    let mut p = P { s, b: s.as_bytes(), i: 0 };
    p.value()
}

// ---------- sanitizing ----------

// Strip C0 controls, DEL and C1 controls (which include ESC, BEL, CSI and OSC
// introducers), and cap the length.
fn safe(s: &str, max: usize) -> String {
    s.chars()
        .filter(|c| !c.is_control())
        .take(max)
        .collect()
}

// Branch (or short hash) from the contents of .git/HEAD. The file is
// untrusted: keep only characters valid in git ref names.
fn branch_from_head(head: &str) -> Option<String> {
    let s = head.trim();
    let (raw, max) = match s.strip_prefix("ref: refs/heads/") {
        Some(r) => (r, MAX_BRANCH),
        None => (s, SHORT_HASH),
    };
    let b: String = raw
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || "-_./+@".contains(*c))
        .take(max)
        .collect();
    if b.is_empty() {
        None
    } else {
        Some(b)
    }
}

// ---------- formatting ----------

fn pick<'a>(pct: i64, low: &'a str, mid: &'a str, high: &'a str) -> &'a str {
    if pct >= 80 {
        high
    } else if pct >= 50 {
        mid
    } else {
        low
    }
}

// Time until the rate-limit window resets, "2h13m" or "45m". None if past.
fn fmt_reset(resets_at: i64, now: i64) -> Option<String> {
    let secs = resets_at - now;
    if secs <= 0 {
        return None;
    }
    let mins = secs / 60;
    let (h, m) = (mins / 60, mins % 60);
    Some(if h > 0 {
        format!("{h}h{m}m")
    } else {
        format!("{m}m")
    })
}

// Time since the last transcript change, rounded down to IDLE_STEP_MIN:
// "5m", "55m", "1h0m", "2h15m". None before the first step (or if the
// file's mtime is in the future).
fn fmt_idle(last_change: i64, now: i64) -> Option<String> {
    let mins = (now - last_change) / 60 / IDLE_STEP_MIN * IDLE_STEP_MIN;
    if mins <= 0 {
        return None;
    }
    let (h, m) = (mins / 60, mins % 60);
    Some(if h > 0 {
        format!("{h}h{m}m")
    } else {
        format!("{m}m")
    })
}

fn tier_abbrev(s: &str) -> String {
    match s {
        "enterprise" => "ent".into(),
        other => other.chars().take(4).collect::<String>().to_lowercase(),
    }
}

fn effort_abbrev(e: &str) -> &str {
    match e {
        "low" => "lo",
        "medium" => "med",
        "high" => "hi",
        "xhigh" => "xhi",
        other => other,
    }
}

// Profile name: basename of CLAUDE_CONFIG_DIR, "default" if unset/empty.
fn profile_name(config_dir: Option<&str>) -> String {
    config_dir
        .and_then(|p| p.trim_end_matches('/').rsplit('/').next())
        .filter(|s| !s.is_empty())
        .unwrap_or("default")
        .to_string()
}

// ---------- rendering ----------

// Everything render() needs from outside stdin, so tests can supply it.
struct Env {
    config_dir: Option<String>,
    now: i64,
    // .git/HEAD contents for a project dir
    read_head: fn(&str) -> Option<String>,
    // subscription tier for a config dir
    read_tier: fn(Option<&str>) -> Option<String>,
    // modification time (unix seconds) of a file, for the transcript
    read_mtime: fn(&str) -> Option<i64>,
}

const CYAN: &str = "\x1b[36m";
const MAGENTA: &str = "\x1b[35m";
const RESET: &str = "\x1b[0m";
const CTX_LOW: &str = "\x1b[32m";
const CTX_MID: &str = "\x1b[33m";
const CTX_HIGH: &str = "\x1b[31m";
const SESS_LOW: &str = "\x1b[94m";
const SESS_MID: &str = "\x1b[95m";
const SESS_HIGH: &str = "\x1b[91m";
const YELLOW: &str = "\x1b[33m";
const DIM: &str = "\x1b[2m";
const PASTEL_ORANGE: &str = "\x1b[38;5;216m";

fn num(v: Option<&Val>) -> Option<f64> {
    v.and_then(|v| v.as_f64())
}

fn render(input: &str, env: &Env) -> String {
    let root = parse(input).unwrap_or(Val::Null);
    let cw = root.get("context_window");
    let five = root.get("rate_limits").and_then(|r| r.get("five_hour"));

    let project_dir = root
        .get("workspace")
        .and_then(|w| w.get("project_dir"))
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let dir_name = safe(
        project_dir.rsplit('/').next().unwrap_or(project_dir),
        MAX_TEXT,
    );

    let ctx_used = num(cw.and_then(|w| w.get("used_percentage"))).unwrap_or(0.0) as i64;
    let ctx_remain = num(cw.and_then(|w| w.get("remaining_percentage"))).unwrap_or(100.0) as i64;
    let ti = num(cw.and_then(|w| w.get("total_input_tokens"))).unwrap_or(0.0);
    let to = num(cw.and_then(|w| w.get("total_output_tokens"))).unwrap_or(0.0);
    let ctx_tokens_k = ((ti + to + 500.0) / 1000.0) as i64;

    let sess_used = num(five.and_then(|f| f.get("used_percentage"))).map(|n| n as i64);
    let sess_resets_at = num(five.and_then(|f| f.get("resets_at"))).map(|n| n as i64);

    let branch = (env.read_head)(project_dir).and_then(|h| branch_from_head(&h));

    let model = root
        .get("model")
        .and_then(|m| {
            m.get("display_name")
                .and_then(|v| v.as_str())
                .or_else(|| m.get("id").and_then(|v| v.as_str()))
        })
        .map(|m| safe(m, MAX_TEXT))
        .filter(|m| !m.is_empty());

    let effort = root
        .get("effort")
        .and_then(|e| e.get("level"))
        .and_then(|v| v.as_str())
        .map(|e| safe(effort_abbrev(e), MAX_TEXT))
        .filter(|e| !e.is_empty());

    let profile = safe(&profile_name(env.config_dir.as_deref()), MAX_TEXT);

    let ctx_color = if ctx_tokens_k >= CTX_TOKEN_WARN_K {
        CTX_HIGH
    } else {
        pick(ctx_used, CTX_LOW, CTX_MID, CTX_HIGH)
    };

    let mut out = format!("{MAGENTA}{profile}{RESET} {CYAN}{dir_name}{RESET}");
    if let Some(br) = branch {
        out += &format!(" {YELLOW}{br}{RESET}");
    }
    out += &format!(" | {ctx_color}c:{ctx_tokens_k}k:{ctx_used}%/{ctx_remain}%{RESET}");
    if let Some(su) = sess_used {
        let sc = pick(su, SESS_LOW, SESS_MID, SESS_HIGH);
        out += &format!(" | {sc}s:{su}%/{}%{RESET}", 100 - su);
        if let Some(rs) = sess_resets_at.and_then(|r| fmt_reset(r, env.now)) {
            out += &format!(" {DIM}{rs}{RESET}");
        }
    } else if let Some(tier) = (env.read_tier)(env.config_dir.as_deref())
        .map(|t| safe(&t, MAX_TEXT))
        .filter(|t| !t.is_empty())
    {
        out += &format!(" | {DIM}{tier}{RESET}");
    }
    if let Some(m) = model {
        out += &format!(" | {DIM}{m}{RESET}");
    }
    if let Some(e) = effort {
        out += &format!(" {DIM}{e}{RESET}");
    }
    if let Some(idle) = root
        .get("transcript_path")
        .and_then(|v| v.as_str())
        .and_then(env.read_mtime)
        .and_then(|t| fmt_idle(t, env.now))
    {
        out += &format!(" | {PASTEL_ORANGE}idle {idle}{RESET}");
    }
    out
}

// ---------- real I/O ----------

fn read_head(project_dir: &str) -> Option<String> {
    fs::read_to_string(format!("{project_dir}/.git/HEAD")).ok()
}

// Falls back to the active profile's subscription tier when Anthropic
// doesn't include `rate_limits.five_hour` (enterprise / usage-based seats).
fn read_tier(config_dir: Option<&str>) -> Option<String> {
    let dir = config_dir
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| std::env::var("HOME").ok().map(|h| format!("{h}/.claude")))?;
    let raw = fs::read_to_string(format!("{dir}/.credentials.json")).ok()?;
    let v = parse(&raw)?;
    let tier = v.get("claudeAiOauth")?.get("subscriptionType")?.as_str()?;
    Some(tier_abbrev(tier))
}

fn read_mtime(path: &str) -> Option<i64> {
    let t = fs::metadata(path).ok()?.modified().ok()?;
    Some(t.duration_since(UNIX_EPOCH).ok()?.as_secs() as i64)
}

// Claude Code's JSON is ~1-4 KiB; pre-size so stdin is one read, not a
// 32/32/64/128/... probe-and-grow sequence.
const STDIN_CAP: usize = 16 * 1024;

#[cfg(not(test))]
#[no_mangle]
pub extern "C" fn main(_argc: i32, _argv: *const *const u8) -> i32 {
    run();
    0
}

fn run() {
    let mut input = String::with_capacity(STDIN_CAP);
    let _ = io::stdin().lock().read_to_string(&mut input);
    let env = Env {
        config_dir: std::env::var("CLAUDE_CONFIG_DIR").ok(),
        now: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0),
        read_head,
        read_tier,
        read_mtime,
    };
    // Flush explicitly: with no_main, std's exit-time flush never runs, and
    // stdout is line-buffered while the line has no trailing newline.
    let mut out = io::stdout().lock();
    let _ = out.write_all(render(&input, &env).as_bytes());
    let _ = out.flush();
}

#[cfg(test)]
mod tests;
