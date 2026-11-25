use crate::codex::*;
use crate::git::find_head;
use crate::json::*;
use crate::status::*;
use crate::tests::{head_evil, head_main, no_head, only_sgr_escapes, plain, NOW};

const SNAPSHOT: &str = include_str!("../tests/fixtures/codex-snapshot.json");

fn fixed_now() -> i64 {
    NOW
}

fn server(profile: &str, read_head: fn(&str) -> Option<String>) -> Server {
    Server::new(profile.into(), fixed_now, read_head)
}

#[test]
fn test_codex_status() {
    struct Case {
        description: &'static str,
        profile: &'static str,
        read_head: fn(&str) -> Option<String>,
        snapshot: &'static str,
        expected: &'static str,
    }
    let cases = [
        Case {
            description: "empty snapshot: defaults",
            profile: "",
            read_head: no_head,
            snapshot: "{}",
            expected: "default unknown | c:0k:0%/100%",
        },
        Case {
            description: "not JSON: defaults",
            profile: "",
            read_head: no_head,
            snapshot: "{oops",
            expected: "default unknown | c:0k:0%/100%",
        },
        Case {
            description: "realistic snapshot: codex limit picked over codex_other",
            profile: "personal",
            read_head: head_main,
            snapshot: SNAPSHOT,
            expected: "personal proj main | c:85k:31%/69% | s:31%/69% 2h13m | GPT-5.5 hi",
        },
        Case {
            description: "no codex limit_id: first limit used",
            profile: "p",
            read_head: no_head,
            snapshot: r#"{"account":{"rate_limits":[{"limit_id":"x","primary_limit":{"used_percent":55,"resets_at":null}}]}}"#,
            expected: "p unknown | c:0k:0%/100% | s:55%/45%",
        },
        Case {
            description: "used_percent over 100: remaining clamps at 0",
            profile: "p",
            read_head: no_head,
            snapshot: r#"{"account":{"rate_limits":[{"limit_id":"codex","primary_limit":{"used_percent":120.0}}]}}"#,
            expected: "p unknown | c:0k:0%/100% | s:120%/0%",
        },
        Case {
            description: "secondary_limit at 50%: weekly shown",
            profile: "p",
            read_head: no_head,
            snapshot: r#"{"account":{"rate_limits":[{"limit_id":"codex","primary_limit":{"used_percent":5},"secondary_limit":{"used_percent":50.0,"window_minutes":10080}}]}}"#,
            expected: "p unknown | c:0k:0%/100% | s:5%/95% | w:50%/50%",
        },
        Case {
            description: "secondary_limit only: weekly without s",
            profile: "p",
            read_head: no_head,
            snapshot: r#"{"account":{"rate_limits":[{"limit_id":"codex","primary_limit":null,"secondary_limit":{"used_percent":91}}]}}"#,
            expected: "p unknown | c:0k:0%/100% | w:91%/9%",
        },
        Case {
            description: "primary_limit null: no s segment",
            profile: "p",
            read_head: no_head,
            snapshot: r#"{"account":{"rate_limits":[{"limit_id":"codex","primary_limit":null}]}}"#,
            expected: "p unknown | c:0k:0%/100%",
        },
        Case {
            description: "rate_limits empty: no s segment",
            profile: "p",
            read_head: no_head,
            snapshot: r#"{"account":{"rate_limits":[]}}"#,
            expected: "p unknown | c:0k:0%/100%",
        },
        Case {
            description: "config.profile used when no --profile/CODEX_HOME",
            profile: "",
            read_head: no_head,
            snapshot: r#"{"config":{"profile":"work"}}"#,
            expected: "work unknown | c:0k:0%/100%",
        },
        Case {
            description: "--profile wins over config.profile",
            profile: "personal",
            read_head: no_head,
            snapshot: r#"{"config":{"profile":"work"}}"#,
            expected: "personal unknown | c:0k:0%/100%",
        },
        Case {
            description: "model id fallback, minimal effort, trailing slash cwd",
            profile: "p",
            read_head: no_head,
            snapshot: r#"{"thread":{"workspace":{"cwd":"/a/b/"},"model":{"display_name":"","id":"gpt-x","reasoning_level":"minimal"}}}"#,
            expected: "p b | c:0k:0%/100% | gpt-x min",
        },
        Case {
            description: "token rounding: 500 -> 1k boundary",
            profile: "p",
            read_head: no_head,
            snapshot: r#"{"thread":{"usage":{"context":{"used_tokens":500}}}}"#,
            expected: "p unknown | c:1k:0%/100%",
        },
        Case {
            description: "token rounding: 499 -> 0k",
            profile: "p",
            read_head: no_head,
            snapshot: r#"{"thread":{"usage":{"context":{"used_tokens":499}}}}"#,
            expected: "p unknown | c:0k:0%/100%",
        },
        Case {
            description: "escape injection via cwd, model, effort, branch",
            profile: "p",
            read_head: head_evil,
            snapshot: "{\"thread\":{\"workspace\":{\"cwd\":\"/x/d\\u001b]0;t\\u0007ir\"},\
                       \"model\":{\"display_name\":\"M\\u001b[2J\\u009b1m\",\"reasoning_level\":\"\\u001b]52;c;eA==\\u0007\"}}}",
            expected: "p d]0;tir a52cZXZpbAb | c:0k:0%/100% | M[2J1m ]52;c;eA==",
        },
    ];
    for c in cases {
        let mut s = server(c.profile, c.read_head);
        let v = parse(c.snapshot).unwrap_or(Val::Null);
        let ansi = s.status(&v, NOW).ansi();
        assert!(only_sgr_escapes(&ansi), "{}: stray escape in {ansi:?}", c.description);
        assert_eq!(plain(&ansi), c.expected, "{}", c.description);
    }
}

#[test]
fn test_codex_context_colour() {
    struct Case {
        description: &'static str,
        used_tokens: i64,
        used_percentage: i64,
        expected: Tone,
    }
    let cases = [
        Case { description: "low", used_tokens: 10_000, used_percentage: 49, expected: Tone::CtxLow },
        Case { description: "mid boundary", used_tokens: 10_000, used_percentage: 50, expected: Tone::CtxMid },
        Case { description: "high boundary", used_tokens: 10_000, used_percentage: 80, expected: Tone::CtxHigh },
        Case { description: "180k forces red", used_tokens: 179_500, used_percentage: 40, expected: Tone::CtxHigh },
        Case { description: "just under 180k", used_tokens: 179_499, used_percentage: 40, expected: Tone::CtxLow },
    ];
    for c in cases {
        let snap = format!(
            r#"{{"thread":{{"usage":{{"context":{{"used_tokens":{},"used_percentage":{}}}}}}}}}"#,
            c.used_tokens, c.used_percentage
        );
        let st = server("p", no_head).status(&parse(&snap).unwrap(), NOW);
        let tone = st.spans().into_iter().find_map(|s| match s {
            Span::Text(t, text) if text.starts_with("c:") => Some(t),
            _ => None,
        });
        assert_eq!(tone, Some(c.expected), "{}", c.description);
    }
}

fn idle_snap(id: &str, state: &str, input_tokens: i64) -> String {
    format!(
        r#"{{"thread":{{"id":"{id}","agent":{{"state":"{state}"}},"usage":{{"total":{{"total_input_tokens":{input_tokens},"output_tokens":1}}}}}}}}"#
    )
}

#[test]
fn test_codex_idle() {
    struct Step {
        description: &'static str,
        id: &'static str,
        state: &'static str,
        input_tokens: i64,
        at_min: i64,
        expected: Option<&'static str>,
    }
    // One server across all steps: idle is tracked between reads.
    let steps = [
        Step { description: "first sight: no history yet", id: "t1", state: "ready", input_tokens: 100, at_min: 0, expected: None },
        Step { description: "unchanged, under 5m", id: "t1", state: "ready", input_tokens: 100, at_min: 4, expected: None },
        Step { description: "unchanged 12m: 10m", id: "t1", state: "ready", input_tokens: 100, at_min: 12, expected: Some("10m") },
        Step { description: "other thread is independent", id: "t2", state: "ready", input_tokens: 5, at_min: 12, expected: None },
        Step { description: "tokens changed: reset", id: "t1", state: "ready", input_tokens: 200, at_min: 13, expected: None },
        Step { description: "5m after the change", id: "t1", state: "ready", input_tokens: 200, at_min: 18, expected: Some("5m") },
        Step { description: "working: hidden", id: "t1", state: "working", input_tokens: 200, at_min: 30, expected: None },
        Step { description: "long turn, tokens flat: still hidden", id: "t1", state: "working", input_tokens: 200, at_min: 50, expected: None },
        Step { description: "back to ready: counts from the last active read", id: "t1", state: "ready", input_tokens: 200, at_min: 51, expected: None },
        Step { description: "waiting (approval) is a change, then idle", id: "t1", state: "waiting", input_tokens: 200, at_min: 51, expected: None },
        Step { description: "waiting 10m", id: "t1", state: "waiting", input_tokens: 200, at_min: 61, expected: Some("10m") },
        Step { description: "over an hour", id: "t1", state: "waiting", input_tokens: 200, at_min: 51 + 67, expected: Some("1h5m") },
        Step { description: "t2 kept its own clock", id: "t2", state: "ready", input_tokens: 5, at_min: 30, expected: Some("15m") },
    ];
    let mut s = server("p", no_head);
    for st in steps {
        let json = idle_snap(st.id, st.state, st.input_tokens);
        let snap = parse(&json).unwrap();
        let got = s.status(&snap, NOW + st.at_min * 60).idle;
        assert_eq!(got.as_deref(), st.expected, "{}", st.description);
    }
}

#[test]
fn test_codex_handle() {
    struct Case {
        description: &'static str,
        request: String,
        expected: Option<&'static [&'static str]>, // substrings of the reply
    }
    let read = |uri: &str| {
        format!(
            r#"{{"jsonrpc":"2.0","id":7,"method":"resources/read","params":{{"uri":"{uri}","_meta":{{"com.openai/codex-statusline-state":{SNAPSHOT}}}}}}}"#
        )
    };
    let cases = [
        Case {
            description: "initialize",
            request: r#"{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}"#.into(),
            expected: Some(&[r#""id":0,"result":{"protocolVersion":"2025-06-18""#, r#""resources":{}"#]),
        },
        Case {
            description: "notification: no reply",
            request: r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#.into(),
            expected: None,
        },
        Case {
            description: "ping with a string id is echoed",
            request: r#"{"jsonrpc":"2.0","id":"a\"b","method":"ping"}"#.into(),
            expected: Some(&[r#"{"jsonrpc":"2.0","id":"a\"b","result":{}}"#]),
        },
        Case {
            description: "tools/list advertises the statusline entrypoint",
            request: r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#.into(),
            expected: Some(&[
                r#""name":"statusline""#,
                r#""entrypoints":[{"type":"statusline","resourceUri":"ui://claude-statusline/footer"}]"#,
            ]),
        },
        Case {
            description: "tools/call",
            request: r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"statusline"}}"#.into(),
            expected: Some(&[r#""result":{"content":[{"type":"text""#]),
        },
        Case {
            description: "tools/call unknown tool",
            request: r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"x"}}"#.into(),
            expected: Some(&[r#""error":{"code":-32602"#]),
        },
        Case {
            description: "resources/list",
            request: r#"{"jsonrpc":"2.0","id":3,"method":"resources/list"}"#.into(),
            expected: Some(&[r#""uri":"ui://claude-statusline/footer","mimeType":"application/json""#]),
        },
        Case {
            description: "resources/templates/list",
            request: r#"{"jsonrpc":"2.0","id":4,"method":"resources/templates/list"}"#.into(),
            expected: Some(&[r#""result":{"resourceTemplates":[]}"#]),
        },
        Case {
            description: "resources/read",
            request: read(URI),
            expected: Some(&[
                r#""id":7,"result":{"contents":[{"uri":"ui://claude-statusline/footer","mimeType":"application/json","text":"{\"type\":\"row\""#,
                "GPT-5.5",
            ]),
        },
        Case {
            description: "resources/read without a snapshot still renders",
            request: format!(r#"{{"jsonrpc":"2.0","id":8,"method":"resources/read","params":{{"uri":"{URI}"}}}}"#),
            expected: Some(&[r#""id":8,"result":{"contents""#, "unknown"]),
        },
        Case {
            description: "resources/read unknown uri",
            request: read("ui://other/footer"),
            expected: Some(&[r#""id":7,"error":{"code":-32602"#]),
        },
        Case {
            description: "unknown method",
            request: r#"{"jsonrpc":"2.0","id":5,"method":"prompts/list"}"#.into(),
            expected: Some(&[r#""id":5,"error":{"code":-32601"#]),
        },
        Case {
            description: "id without method",
            request: r#"{"jsonrpc":"2.0","id":6}"#.into(),
            expected: Some(&[r#""id":6,"error":{"code":-32600"#]),
        },
        Case {
            description: "not JSON",
            request: "{garbage".into(),
            expected: Some(&[r#"{"jsonrpc":"2.0","id":null,"error":{"code":-32700"#]),
        },
    ];
    for c in cases {
        let got = server("personal", head_main).handle(&c.request);
        match (got, c.expected) {
            (None, None) => {}
            (Some(r), Some(want)) => {
                assert!(parse(&r).is_some(), "{}: reply isn't JSON: {r}", c.description);
                assert!(!r.contains('\n'), "{}: reply must be one line", c.description);
                for w in want {
                    assert!(r.contains(w), "{}: {w:?} not in {r}", c.description);
                }
            }
            (got, _) => panic!("{}: got {got:?}", c.description),
        }
    }
}

// Non-null nodes, max depth and leaf text bytes of a UiTree.
fn tree_stats(v: &Val, depth: usize, nodes: &mut usize, max_depth: &mut usize, text: &mut usize) {
    match v {
        Val::Null => return,
        Val::Str(s) => *text += s.len(),
        _ => {}
    }
    *nodes += 1;
    *max_depth = (*max_depth).max(depth);
    if let Some(Val::Arr(children)) = v.get("children") {
        for c in children {
            tree_stats(c, depth + 1, nodes, max_depth, text);
        }
    }
}

#[test]
fn test_ui_tree() {
    let long = "x".repeat(MAX_TEXT);
    struct Case {
        description: &'static str,
        status: Status,
        expected: Option<&'static str>, // exact JSON, when given
    }
    let cases = [
        Case {
            description: "minimal line",
            status: Status {
                profile: "p".into(),
                dir: "d".into(),
                branch: None,
                ctx_k: 1,
                ctx_used: 2,
                ctx_remain: 98,
                session: None,
                week: None,
                tier: None,
                model: None,
                effort: None,
                idle: None,
            },
            expected: Some(concat!(
                r#"{"type":"row","props":{"gap":1,"wrap":"nowrap"},"children":["#,
                r#"{"type":"text","props":{"color":"magenta"},"children":["p"]},"#,
                r#"{"type":"text","props":{"color":"cyan"},"children":["d"]},"#,
                r##"{"type":"text","props":{"color":"#8a8a8a"},"children":["|"]},"##,
                r#"{"type":"text","props":{"color":"green"},"children":["c:1k:2%/98%"]}]}"#
            )),
        },
        Case {
            description: "every segment at max length, quotes in text",
            status: Status {
                profile: long.clone(),
                dir: format!("\"{long}\\"),
                branch: Some("x".repeat(MAX_BRANCH)),
                ctx_k: 999,
                ctx_used: 100,
                ctx_remain: 0,
                session: Some(Limit { used: 100, remain: 0, reset: Some("4h59m".into()) }),
                week: Some(Limit { used: 100, remain: 0, reset: None }),
                tier: Some(long.clone()),
                model: Some(long.clone()),
                effort: Some(long.clone()),
                idle: Some("99h55m".into()),
            },
            expected: None,
        },
    ];
    for c in cases {
        let tree = c.status.ui_tree();
        if let Some(want) = c.expected {
            assert_eq!(tree, want, "{}", c.description);
        }
        let v = parse(&tree).unwrap_or_else(|| panic!("{}: not JSON: {tree}", c.description));
        let (mut nodes, mut depth, mut text) = (0, 0, 0);
        tree_stats(&v, 0, &mut nodes, &mut depth, &mut text);
        // Codex limits: 64 KiB JSON, 256 non-null nodes, depth 16, 16 KiB text.
        assert!(tree.len() <= 64 * 1024, "{}: {} bytes", c.description, tree.len());
        assert!(
            nodes <= 256 && depth <= 16 && text <= 16 * 1024,
            "{}: {nodes} nodes, depth {depth}, {text} B text",
            c.description
        );
        // The leaves joined by the row's 1-column gap are the plain ANSI line.
        let mut leaves = Vec::new();
        if let Some(Val::Arr(items)) = v.get("children") {
            for item in items {
                if let Some(Val::Arr(ch)) = item.get("children") {
                    leaves.extend(ch.iter().filter_map(|s| s.as_str().map(str::to_string)));
                }
            }
        }
        assert_eq!(leaves.join(" "), plain(&c.status.ansi()), "{}: same text as ANSI", c.description);
    }
}

#[test]
fn test_push_str_round_trip() {
    struct Case {
        description: &'static str,
        input: &'static str,
        expected: &'static str, // JSON literal
    }
    let cases = [
        Case { description: "plain", input: "abc", expected: r#""abc""# },
        Case { description: "empty", input: "", expected: r#""""# },
        Case { description: "quote and backslash", input: "a\"b\\c", expected: r#""a\"b\\c""# },
        Case { description: "newline, tab, CR", input: "a\nb\tc\rd", expected: r#""a\nb\tc\rd""# },
        Case { description: "other C0 as \\u", input: "\x1b[0m\x07", expected: r#""\u001b[0m\u0007""# },
        Case { description: "unicode passes through", input: "café-日本", expected: r#""café-日本""# },
    ];
    for c in cases {
        let mut out = String::new();
        push_str(&mut out, c.input);
        assert_eq!(out, c.expected, "{}", c.description);
        let back = parse(&out);
        assert_eq!(back.as_ref().and_then(|v| v.as_str()), Some(c.input), "{}: round trip", c.description);
    }
}

#[test]
fn test_push_val() {
    struct Case {
        description: &'static str,
        input: &'static str,
        expected: &'static str,
    }
    let cases = [
        Case { description: "integer id", input: "42", expected: "42" },
        Case { description: "negative", input: "-1", expected: "-1" },
        Case { description: "fraction", input: "1.5", expected: "1.5" },
        Case { description: "string id", input: r#""req-1""#, expected: r#""req-1""# },
        Case { description: "null", input: "null", expected: "null" },
        Case { description: "nested", input: r#"{"a":[true,false,null],"b":{}}"#, expected: r#"{"a":[true,false,null],"b":{}}"# },
    ];
    for c in cases {
        let mut out = String::new();
        push_val(&mut out, &parse(c.input).unwrap());
        assert_eq!(out, c.expected, "{}", c.description);
    }
}

#[test]
fn test_find_head() {
    use std::fs;
    let tmp = std::env::temp_dir();
    let root = tmp.join(format!("cs-find-head-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let mk = |p: &str, body: &str| {
        let f = root.join(p);
        fs::create_dir_all(f.parent().unwrap()).unwrap();
        fs::write(f, body).unwrap();
    };
    mk("repo/.git/HEAD", "ref: refs/heads/main\n");
    fs::create_dir_all(root.join("repo/src/deep")).unwrap();
    mk("repo/.git/worktrees/wt/HEAD", "ref: refs/heads/feature\n");
    mk("repo/.claude/worktrees/wt/.git", "gitdir: ../../../.git/worktrees/wt\n");
    mk("abs/.git", &format!("gitdir: {}\n", root.join("repo/.git/worktrees/wt").display()));
    mk("bad/.git", "not a gitdir line\n");

    let main = Some("ref: refs/heads/main\n".to_string());
    let feature = Some("ref: refs/heads/feature\n".to_string());
    struct Case {
        description: &'static str,
        dir: String,
        expected: Option<String>,
    }
    let at = |p: &str| root.join(p).display().to_string();
    let cases = [
        Case { description: "repo root", dir: at("repo"), expected: main.clone() },
        Case { description: "subdirectory walks up", dir: at("repo/src/deep"), expected: main.clone() },
        Case { description: "worktree .git file, relative gitdir", dir: at("repo/.claude/worktrees/wt"), expected: feature.clone() },
        Case { description: "absolute gitdir", dir: at("abs"), expected: feature.clone() },
        Case { description: "malformed .git file stops the walk", dir: at("bad"), expected: None },
        // Walks past our tree, so it finds whatever temp_dir itself would.
        Case { description: "no repo: same as the temp dir's answer", dir: at("nope/x"), expected: find_head(&tmp.display().to_string()) },
        Case { description: "relative path refused", dir: "repo".into(), expected: None },
        Case { description: "empty", dir: String::new(), expected: None },
    ];
    for c in cases {
        assert_eq!(find_head(&c.dir), c.expected, "{}", c.description);
    }
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn test_profile_arg() {
    struct Case {
        description: &'static str,
        args: &'static [&'static str],
        expected: &'static str,
    }
    // Only --profile cases: CODEX_HOME comes from the real environment.
    let cases = [
        Case { description: "--profile NAME", args: &["--profile", "personal"], expected: "personal" },
        Case { description: "--profile path uses basename", args: &["--profile", "/h/.codex/profiles/work/"], expected: "work" },
        Case { description: "escape in name stripped", args: &["--profile", "a\x1b]0;xb"], expected: "a]0;xb" },
    ];
    for c in cases {
        assert_eq!(profile(c.args), c.expected, "{}", c.description);
    }
}

// In-process cost of one resources/read (parse request + snapshot, build
// Status, render UiTree, encode reply):
//   cargo test --release -- --ignored --nocapture bench_codex_read
#[test]
#[ignore]
fn bench_codex_read() {
    let req = format!(
        r#"{{"jsonrpc":"2.0","id":7,"method":"resources/read","params":{{"uri":"{URI}","_meta":{{"com.openai/codex-statusline-state":{SNAPSHOT}}}}}}}"#
    );
    let mut s = server("personal", head_main);
    let n = 200_000;
    let t = std::time::Instant::now();
    let mut bytes = 0;
    for _ in 0..n {
        bytes += std::hint::black_box(s.handle(std::hint::black_box(&req))).map_or(0, |r| r.len());
    }
    let ns = t.elapsed().as_nanos() / n as u128;
    println!("codex read: {ns} ns/iter ({} B in, {} B out)", req.len(), bytes / n);
}
