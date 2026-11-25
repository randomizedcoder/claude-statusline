// Codex front-end: a long-running stdio MCP server that Codex polls about once
// a second with resources/read. Each read carries the TUI's statusline
// snapshot in params._meta["com.openai/codex-statusline-state"]; the reply is
// the same Status rendered as a UiTree.
//
// Protocol (codex-rs/skills/.../create-statusline): newline-delimited
// JSON-RPC 2.0; one tool whose _meta["openai/ui"].entrypoints names a ui://
// statusline resource; resources/read returns exactly one application/json
// text content at that URI. Codex stops polling after any error or a read
// slower than 1s, so a bad snapshot renders defaults instead of erroring.
//
// Being long-running, there's no per-refresh exec cost, and idle time is
// tracked in memory (Codex has no transcript path to stat).
//
// Snapshot -> Status:
//   profile  --profile NAME, else basename of $CODEX_HOME (only passed through
//            if listed in the server's env_vars), else config.profile
//   dir      thread.workspace.cwd basename; branch: nearest .git/HEAD above it
//   c:       thread.usage.context.{used_tokens, used_percentage}
//   s:       account.rate_limits[limit_id "codex"].primary_limit
//   w:       the same entry's secondary_limit (the weekly window)
//   model    thread.model.display_name [reasoning_level]
//   idle     time since thread.agent.state or token counts last changed,
//            hidden while the agent is starting/working/thinking

use crate::json::{self, parse, Val};
use crate::status::{
    base_name, branch_from_head, effort_abbrev, fmt_idle, safe, Limit, Status, MAX_TEXT,
};
use std::io::{self, BufRead, Write};

pub(crate) const URI: &str = "ui://claude-statusline/footer";
pub(crate) const TOOL: &str = "statusline";
const STATE_KEY: &str = "com.openai/codex-statusline-state";
const PROTOCOL_VERSION: &str = "2025-06-18";
// Threads whose idle state is remembered (Codex shows one at a time).
const MAX_THREADS: usize = 32;

struct Seen {
    id: String,
    state: String,
    tokens: (i64, i64),
    last_change: i64,
}

pub(crate) struct Server {
    pub profile: String,
    pub now: fn() -> i64,
    // .git/HEAD contents for the repo containing a directory
    pub read_head: fn(&str) -> Option<String>,
    seen: Vec<Seen>,
}

fn num(v: Option<&Val>) -> Option<f64> {
    v.and_then(|v| v.as_f64())
}

fn text(v: Option<&Val>) -> Option<String> {
    v.and_then(|v| v.as_str())
        .map(|s| safe(s, MAX_TEXT))
        .filter(|s| !s.is_empty())
}

fn active(state: &str) -> bool {
    matches!(state, "starting" | "working" | "thinking")
}

impl Server {
    pub(crate) fn new(
        profile: String,
        now: fn() -> i64,
        read_head: fn(&str) -> Option<String>,
    ) -> Self {
        Server { profile, now, read_head, seen: Vec::new() }
    }

    // Time since this thread's state or token counts last changed.
    fn idle(&mut self, thread: Option<&Val>, now: i64) -> Option<String> {
        let get = |k: &str| thread.and_then(|t| t.get(k));
        let id = get("id").and_then(|v| v.as_str()).unwrap_or("");
        let state = get("agent").and_then(|a| a.get("state")).and_then(|v| v.as_str()).unwrap_or("");
        let usage = get("usage");
        let total = usage.and_then(|u| u.get("total"));
        let t = |k: &str| num(total.and_then(|t| t.get(k))).unwrap_or(0.0) as i64;
        let tokens = (
            t("total_input_tokens") + t("output_tokens"),
            num(usage.and_then(|u| u.get("context")).and_then(|c| c.get("used_tokens")))
                .unwrap_or(0.0) as i64,
        );

        let last_change = match self.seen.iter_mut().find(|s| s.id == id) {
            Some(s) => {
                if active(state) || s.state != state || s.tokens != tokens {
                    s.state.clear();
                    s.state.push_str(state);
                    s.tokens = tokens;
                    s.last_change = now;
                }
                s.last_change
            }
            None => {
                if self.seen.len() >= MAX_THREADS {
                    self.seen.remove(0);
                }
                self.seen.push(Seen { id: id.into(), state: state.into(), tokens, last_change: now });
                now
            }
        };
        if active(state) {
            None
        } else {
            fmt_idle(last_change, now)
        }
    }

    pub(crate) fn status(&mut self, snap: &Val, now: i64) -> Status {
        let thread = snap.get("thread");
        let get = |k: &str| thread.and_then(|t| t.get(k));
        let cwd = get("workspace").and_then(|w| w.get("cwd")).and_then(|v| v.as_str());
        let ctx = get("usage").and_then(|u| u.get("context"));
        let model = get("model");

        let limits = snap.get("account").and_then(|a| a.get("rate_limits"));
        let codex_limit = match limits {
            Some(Val::Arr(a)) => a
                .iter()
                .find(|l| l.get("limit_id").and_then(|v| v.as_str()) == Some("codex"))
                .or(a.first()),
            _ => None,
        };
        let window = |k: &str| {
            let w = codex_limit.and_then(|l| l.get(k));
            Limit::from(
                num(w.and_then(|p| p.get("used_percent"))),
                num(w.and_then(|p| p.get("resets_at"))),
                now,
            )
        };

        let used_tokens = num(ctx.and_then(|c| c.get("used_tokens"))).unwrap_or(0.0);
        let ctx_used = num(ctx.and_then(|c| c.get("used_percentage"))).unwrap_or(0.0) as i64;

        let profile = Some(self.profile.clone())
            .filter(|p| !p.is_empty())
            .or_else(|| text(snap.get("config").and_then(|c| c.get("profile"))))
            .unwrap_or_else(|| "default".into());

        Status {
            profile,
            dir: cwd.map(base_name).filter(|d| !d.is_empty()).unwrap_or_else(|| "unknown".into()),
            branch: cwd.and_then(self.read_head).and_then(|h| branch_from_head(&h)),
            ctx_k: ((used_tokens + 500.0) / 1000.0) as i64,
            ctx_used,
            ctx_remain: (100 - ctx_used).max(0),
            session: window("primary_limit"),
            week: window("secondary_limit"),
            tier: None,
            model: model.and_then(|m| text(m.get("display_name")).or_else(|| text(m.get("id")))),
            effort: model
                .and_then(|m| m.get("reasoning_level"))
                .and_then(|v| v.as_str())
                .map(|e| safe(effort_abbrev(e), MAX_TEXT))
                .filter(|e| !e.is_empty()),
            idle: self.idle(thread, now),
        }
    }

    // One JSON-RPC message in, at most one reply line out (None for
    // notifications).
    pub(crate) fn handle(&mut self, line: &str) -> Option<String> {
        let Some(req) = parse(line) else {
            return Some(error(&Val::Null, -32700, "Parse error"));
        };
        // No id: a notification (e.g. notifications/initialized); no reply.
        let id = req.get("id")?;
        let Some(method) = req.get("method").and_then(|m| m.as_str()) else {
            return Some(error(id, -32600, "Invalid request"));
        };
        let params = req.get("params");
        let param = |k: &str| params.and_then(|p| p.get(k)).and_then(|v| v.as_str());

        let result = match method {
            "initialize" => format!(
                r#"{{"protocolVersion":"{PROTOCOL_VERSION}","capabilities":{{"tools":{{}},"resources":{{}}}},"serverInfo":{{"name":"claude-statusline","version":"{}"}}}}"#,
                env!("CARGO_PKG_VERSION")
            ),
            "ping" => "{}".into(),
            "tools/list" => format!(
                r#"{{"tools":[{{"name":"{TOOL}","title":"claude-statusline","description":"The claude-statusline line (profile, dir, branch, context, limits, model, idle) in the Codex footer.","inputSchema":{{"type":"object","properties":{{}}}},"_meta":{{"openai/ui":{{"entrypoints":[{{"type":"statusline","resourceUri":"{URI}"}}]}}}}}}]}}"#
            ),
            "tools/call" if param("name") == Some(TOOL) => format!(
                r#"{{"content":[{{"type":"text","text":"The statusline resource is {URI}."}}]}}"#
            ),
            "tools/call" => return Some(error(id, -32602, "Unknown tool name")),
            "resources/list" => format!(
                r#"{{"resources":[{{"name":"footer","uri":"{URI}","mimeType":"application/json"}}]}}"#
            ),
            "resources/templates/list" => r#"{"resourceTemplates":[]}"#.into(),
            "resources/read" if param("uri") == Some(URI) => {
                let snap = params
                    .and_then(|p| p.get("_meta"))
                    .and_then(|m| m.get(STATE_KEY))
                    .unwrap_or(&Val::Null);
                let tree = self.status(snap, (self.now)()).ui_tree();
                let mut r = String::with_capacity(tree.len() * 2 + 128);
                r.push_str(r#"{"contents":[{"uri":""#);
                r.push_str(URI);
                r.push_str(r#"","mimeType":"application/json","text":"#);
                json::push_str(&mut r, &tree);
                r.push_str("}]}");
                r
            }
            "resources/read" => return Some(error(id, -32602, "Unknown resource URI")),
            _ => return Some(error(id, -32601, "Method not found")),
        };
        let mut out = String::with_capacity(result.len() + 48);
        out.push_str(r#"{"jsonrpc":"2.0","id":"#);
        json::push_val(&mut out, id);
        out.push_str(r#","result":"#);
        out.push_str(&result);
        out.push('}');
        Some(out)
    }
}

fn error(id: &Val, code: i32, message: &str) -> String {
    let mut out = String::from(r#"{"jsonrpc":"2.0","id":"#);
    json::push_val(&mut out, id);
    out.push_str(&format!(r#","error":{{"code":{code},"message":"#));
    json::push_str(&mut out, message);
    out.push_str("}}");
    out
}

// ---------- real I/O ----------

// --profile NAME, else basename of $CODEX_HOME, else "" (then the
// snapshot's config.profile, then "default").
pub(crate) fn profile(args: &[&str]) -> String {
    let from_arg = args
        .windows(2)
        .find(|w| w[0] == "--profile")
        .map(|w| w[1].to_string());
    from_arg
        .or_else(|| std::env::var("CODEX_HOME").ok())
        .map(|p| base_name(&p))
        .unwrap_or_default()
}

pub(crate) fn run(profile: String) {
    let mut server = Server::new(profile, crate::claude::now, crate::git::find_head);
    let mut stdin = io::stdin().lock();
    let mut out = io::stdout().lock();
    let mut buf = Vec::with_capacity(64 * 1024);
    loop {
        buf.clear();
        match stdin.read_until(b'\n', &mut buf) {
            Ok(0) | Err(_) => return, // Codex closed the pipe
            Ok(_) => {}
        }
        let line = String::from_utf8_lossy(&buf);
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(mut reply) = server.handle(line) {
            reply.push('\n'); // one write(2) per reply
            if out.write_all(reply.as_bytes()).is_err() || out.flush().is_err() {
                return;
            }
        }
    }
}
