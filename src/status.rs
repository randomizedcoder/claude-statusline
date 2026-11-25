// The shared core: one Status, built by a front-end (claude.rs from Claude
// Code's stdin JSON, codex.rs from Codex's statusline snapshot), turned into a
// list of spans, and rendered as ANSI (Claude) or a UiTree (Codex).
//
// Every string in a Status comes from outside this program and must already
// have been passed through safe() (or branch_from_head) by the front-end.

use crate::json;

// Force the context (c:) segment red once it reaches this many thousand tokens,
// regardless of percentage. Warns before the 200k mark where per-token cost jumps.
pub(crate) const CTX_TOKEN_WARN_K: i64 = 180;

// Longest branch name shown; detached HEADs show a 7-char hash.
pub(crate) const MAX_BRANCH: usize = 64;
const SHORT_HASH: usize = 7;
// Cap for any other printed string (dir, model, profile, ...).
pub(crate) const MAX_TEXT: usize = 128;

// Idle segment granularity: hidden for the first step, then rounds down to a
// multiple of this, so it changes at most once per step.
pub(crate) const IDLE_STEP_MIN: i64 = 5;

// ---------- sanitizing ----------

// Strip C0 controls, DEL and C1 controls (which include ESC, BEL, CSI and OSC
// introducers), and cap the length.
pub(crate) fn safe(s: &str, max: usize) -> String {
    s.chars().filter(|c| !c.is_control()).take(max).collect()
}

// Branch (or short hash) from the contents of .git/HEAD. The file is
// untrusted: keep only characters valid in git ref names.
pub(crate) fn branch_from_head(head: &str) -> Option<String> {
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

// Last path component ("nixos" for "/home/das/nixos/"), sanitized.
pub(crate) fn base_name(path: &str) -> String {
    let p = path.trim_end_matches('/');
    safe(p.rsplit('/').next().unwrap_or(p), MAX_TEXT)
}

// ---------- formatting ----------

pub(crate) fn pick<T>(pct: i64, low: T, mid: T, high: T) -> T {
    if pct >= 80 {
        high
    } else if pct >= 50 {
        mid
    } else {
        low
    }
}

fn fmt_hm(mins: i64) -> String {
    let (h, m) = (mins / 60, mins % 60);
    if h > 0 {
        format!("{h}h{m}m")
    } else {
        format!("{m}m")
    }
}

// Time until the rate-limit window resets, "2h13m" or "45m". None if past.
pub(crate) fn fmt_reset(resets_at: i64, now: i64) -> Option<String> {
    let secs = resets_at - now;
    (secs > 0).then(|| fmt_hm(secs / 60))
}

// Time since the last change, rounded down to IDLE_STEP_MIN: "5m", "55m",
// "1h0m", "2h15m". None before the first step (or if it's in the future).
pub(crate) fn fmt_idle(last_change: i64, now: i64) -> Option<String> {
    let mins = (now - last_change) / 60 / IDLE_STEP_MIN * IDLE_STEP_MIN;
    (mins > 0).then(|| fmt_hm(mins))
}

pub(crate) fn tier_abbrev(s: &str) -> String {
    match s {
        "enterprise" => "ent".into(),
        other => other.chars().take(4).collect::<String>().to_lowercase(),
    }
}

pub(crate) fn effort_abbrev(e: &str) -> &str {
    match e {
        "minimal" => "min",
        "low" => "lo",
        "medium" => "med",
        "high" => "hi",
        "xhigh" => "xhi",
        other => other,
    }
}

// ---------- the model ----------

pub(crate) struct Limit {
    pub used: i64,
    pub remain: i64,
    pub reset: Option<String>,
}

pub(crate) struct Status {
    pub profile: String,
    pub dir: String,
    pub branch: Option<String>,
    pub ctx_k: i64,
    pub ctx_used: i64,
    pub ctx_remain: i64,
    pub session: Option<Limit>,
    pub tier: Option<String>, // shown only when session is None
    pub model: Option<String>,
    pub effort: Option<String>,
    pub idle: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Tone {
    Profile,
    Dir,
    Branch,
    CtxLow,
    CtxMid,
    CtxHigh,
    SessLow,
    SessMid,
    SessHigh,
    Dim,
    Idle,
}

#[derive(Debug, PartialEq)]
pub(crate) enum Span {
    Text(Tone, String),
    Gap, // one space
    Sep, // " | "
}

impl Status {
    // The line, front-end independent:
    // <profile> <dir> [<branch>] | c:<K>k:U%/R% [| s:U%/R% [<reset>] | <tier>] [| <model>] [<effort>] [| idle <T>]
    pub(crate) fn spans(&self) -> Vec<Span> {
        use Span::*;
        let mut v = Vec::with_capacity(20);
        v.push(Text(Tone::Profile, self.profile.clone()));
        v.push(Gap);
        v.push(Text(Tone::Dir, self.dir.clone()));
        if let Some(b) = &self.branch {
            v.push(Gap);
            v.push(Text(Tone::Branch, b.clone()));
        }
        let ctx_tone = if self.ctx_k >= CTX_TOKEN_WARN_K {
            Tone::CtxHigh
        } else {
            pick(self.ctx_used, Tone::CtxLow, Tone::CtxMid, Tone::CtxHigh)
        };
        v.push(Sep);
        v.push(Text(
            ctx_tone,
            format!("c:{}k:{}%/{}%", self.ctx_k, self.ctx_used, self.ctx_remain),
        ));
        if let Some(s) = &self.session {
            v.push(Sep);
            v.push(Text(
                pick(s.used, Tone::SessLow, Tone::SessMid, Tone::SessHigh),
                format!("s:{}%/{}%", s.used, s.remain),
            ));
            if let Some(r) = &s.reset {
                v.push(Gap);
                v.push(Text(Tone::Dim, r.clone()));
            }
        } else if let Some(t) = &self.tier {
            v.push(Sep);
            v.push(Text(Tone::Dim, t.clone()));
        }
        if let Some(m) = &self.model {
            v.push(Sep);
            v.push(Text(Tone::Dim, m.clone()));
        }
        if let Some(e) = &self.effort {
            v.push(Gap);
            v.push(Text(Tone::Dim, e.clone()));
        }
        if let Some(i) = &self.idle {
            v.push(Sep);
            v.push(Text(Tone::Idle, format!("idle {i}")));
        }
        v
    }

    // Claude Code: one ANSI-coloured line.
    pub(crate) fn ansi(&self) -> String {
        const RESET: &str = "\x1b[0m";
        let mut out = String::with_capacity(256);
        for s in self.spans() {
            match s {
                Span::Text(t, text) => {
                    out.push_str(ansi_code(t));
                    out.push_str(&text);
                    out.push_str(RESET);
                }
                Span::Gap => out.push(' '),
                Span::Sep => out.push_str(" | "),
            }
        }
        out
    }

    // Codex: a UiTree, one row with a 1-column gap between items, so Gap and
    // Sep both come out as single spaces around their text.
    pub(crate) fn ui_tree(&self) -> String {
        let mut out = String::with_capacity(1024);
        out.push_str(r#"{"type":"row","props":{"gap":1,"wrap":"nowrap"},"children":["#);
        let mut first = true;
        for s in self.spans() {
            let (color, text) = match &s {
                Span::Text(t, text) => (ui_color(*t), text.as_str()),
                Span::Sep => (ui_color(Tone::Dim), "|"),
                Span::Gap => continue,
            };
            if !first {
                out.push(',');
            }
            first = false;
            out.push_str(r#"{"type":"text","props":{"color":""#);
            out.push_str(color);
            out.push_str(r#""},"children":["#);
            json::push_str(&mut out, text);
            out.push_str("]}");
        }
        out.push_str("]}");
        out
    }
}

fn ansi_code(t: Tone) -> &'static str {
    match t {
        Tone::Profile => "\x1b[35m",
        Tone::Dir => "\x1b[36m",
        Tone::Branch => "\x1b[33m",
        Tone::CtxLow => "\x1b[32m",
        Tone::CtxMid => "\x1b[33m",
        Tone::CtxHigh => "\x1b[31m",
        Tone::SessLow => "\x1b[94m",
        Tone::SessMid => "\x1b[95m",
        Tone::SessHigh => "\x1b[91m",
        Tone::Dim => "\x1b[2m",
        Tone::Idle => "\x1b[38;5;216m", // pastel orange
    }
}

// Named ANSI colours follow the terminal palette, like the ANSI line does.
// Codex has no "dim", and status-metadata falls back to cyan, so dim is a
// grey (xterm 245); idle is xterm 216, the same pastel orange.
fn ui_color(t: Tone) -> &'static str {
    match t {
        Tone::Profile => "magenta",
        Tone::Dir => "cyan",
        Tone::Branch => "yellow",
        Tone::CtxLow => "green",
        Tone::CtxMid => "yellow",
        Tone::CtxHigh => "red",
        Tone::SessLow => "blue",
        Tone::SessMid => "magenta",
        Tone::SessHigh => "red",
        Tone::Dim => "#8a8a8a",
        Tone::Idle => "#ffaf87",
    }
}
