// Claude Code front-end: the statusline JSON on stdin -> one ANSI line on
// stdout, one process per refresh.

use crate::json::{parse, Val};
use crate::status::{
    base_name, branch_from_head, effort_abbrev, fmt_idle, fmt_reset, safe, tier_abbrev, Limit,
    Status, MAX_TEXT,
};
use std::fs;
use std::io::{self, Read, Write};
use std::time::{SystemTime, UNIX_EPOCH};

// Everything render() needs from outside stdin, so tests can supply it.
pub(crate) struct Env {
    pub config_dir: Option<String>,
    pub now: i64,
    // .git/HEAD contents for a project dir
    pub read_head: fn(&str) -> Option<String>,
    // subscription tier for a config dir
    pub read_tier: fn(Option<&str>) -> Option<String>,
    // modification time (unix seconds) of a file, for the transcript
    pub read_mtime: fn(&str) -> Option<i64>,
}

// Profile name: basename of CLAUDE_CONFIG_DIR, "default" if unset/empty.
pub(crate) fn profile_name(config_dir: Option<&str>) -> String {
    Some(base_name(config_dir.unwrap_or("")))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "default".into())
}

fn num(v: Option<&Val>) -> Option<f64> {
    v.and_then(|v| v.as_f64())
}

fn text(v: Option<&Val>) -> Option<String> {
    v.and_then(|v| v.as_str())
        .map(|s| safe(s, MAX_TEXT))
        .filter(|s| !s.is_empty())
}

pub(crate) fn status(input: &str, env: &Env) -> Status {
    let root = parse(input).unwrap_or(Val::Null);
    let cw = root.get("context_window");
    let five = root.get("rate_limits").and_then(|r| r.get("five_hour"));

    let project_dir = root
        .get("workspace")
        .and_then(|w| w.get("project_dir"))
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");

    let ti = num(cw.and_then(|w| w.get("total_input_tokens"))).unwrap_or(0.0);
    let to = num(cw.and_then(|w| w.get("total_output_tokens"))).unwrap_or(0.0);

    let session = num(five.and_then(|f| f.get("used_percentage"))).map(|u| {
        let used = u as i64;
        Limit {
            used,
            remain: 100 - used,
            reset: num(five.and_then(|f| f.get("resets_at")))
                .and_then(|r| fmt_reset(r as i64, env.now)),
        }
    });
    // Only read the credentials file when there's no rate limit to show.
    let tier = if session.is_none() {
        (env.read_tier)(env.config_dir.as_deref())
            .map(|t| safe(&t, MAX_TEXT))
            .filter(|t| !t.is_empty())
    } else {
        None
    };

    let model = root
        .get("model")
        .and_then(|m| text(m.get("display_name")).or_else(|| text(m.get("id"))));
    let effort = root
        .get("effort")
        .and_then(|e| e.get("level"))
        .and_then(|v| v.as_str())
        .map(|e| safe(effort_abbrev(e), MAX_TEXT))
        .filter(|e| !e.is_empty());

    Status {
        profile: profile_name(env.config_dir.as_deref()),
        dir: base_name(project_dir),
        branch: (env.read_head)(project_dir).and_then(|h| branch_from_head(&h)),
        ctx_k: ((ti + to + 500.0) / 1000.0) as i64,
        ctx_used: num(cw.and_then(|w| w.get("used_percentage"))).unwrap_or(0.0) as i64,
        ctx_remain: num(cw.and_then(|w| w.get("remaining_percentage"))).unwrap_or(100.0) as i64,
        session,
        tier,
        model,
        effort,
        idle: root
            .get("transcript_path")
            .and_then(|v| v.as_str())
            .and_then(env.read_mtime)
            .and_then(|t| fmt_idle(t, env.now)),
    }
}

pub(crate) fn render(input: &str, env: &Env) -> String {
    status(input, env).ansi()
}

// ---------- real I/O ----------

pub(crate) fn read_head(project_dir: &str) -> Option<String> {
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

pub(crate) fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// Claude Code's JSON is ~1-4 KiB; pre-size so stdin is one read, not a
// 32/32/64/128/... probe-and-grow sequence.
const STDIN_CAP: usize = 16 * 1024;

pub(crate) fn run() {
    let mut input = String::with_capacity(STDIN_CAP);
    let _ = io::stdin().lock().read_to_string(&mut input);
    let env = Env {
        config_dir: std::env::var("CLAUDE_CONFIG_DIR").ok(),
        now: now(),
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
