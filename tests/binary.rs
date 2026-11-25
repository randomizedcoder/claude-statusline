// End-to-end: run the built binary as Claude Code does (JSON on stdin, line on
// stdout). Catches what unit tests on render() can't, e.g. output lost at exit.

use std::io::Write;
use std::process::{Command, Stdio};

fn run(input: &str) -> (String, bool) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_claude-statusline"))
        .env_remove("CLAUDE_CONFIG_DIR")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn");
    child.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
    let out = child.wait_with_output().expect("wait");
    (String::from_utf8_lossy(&out.stdout).into_owned(), out.status.success())
}

#[test]
fn test_binary() {
    struct Case {
        description: &'static str,
        input: String,
        expected: &'static [&'static str], // substrings that must appear
    }
    let cases = [
        Case {
            description: "typical input prints model and context",
            input: r#"{"workspace":{"project_dir":"/x/proj"},"context_window":{"used_percentage":42,"remaining_percentage":58,"total_input_tokens":12000,"total_output_tokens":0},"model":{"display_name":"Opus 5.5"}}"#.into(),
            expected: &["proj", "c:12k:42%/58%", "Opus 5.5"],
        },
        Case {
            description: "input larger than the pre-sized stdin buffer",
            input: format!(r#"{{"pad":"{}","model":{{"display_name":"Big"}}}}"#, "x".repeat(64 * 1024)),
            expected: &["Big"],
        },
        Case {
            description: "garbage input still exits 0",
            input: "not json".into(),
            expected: &[],
        },
        Case {
            description: "empty input still exits 0",
            input: String::new(),
            expected: &[],
        },
    ];
    for c in cases {
        let (out, ok) = run(&c.input);
        assert!(ok, "{}: non-zero exit", c.description);
        for want in c.expected {
            assert!(out.contains(want), "{}: {want:?} not in {out:?}", c.description);
        }
    }
}

// Codex mode: a session over real pipes. Requests in, one reply line per
// request (none for the notification), clean exit when stdin closes.
#[test]
fn test_binary_codex() {
    let snapshot = include_str!("fixtures/codex-snapshot.json").trim();
    let session = [
        r#"{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"0"}}}"#.to_string(),
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#.to_string(),
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#.to_string(),
        "".to_string(), // blank lines are skipped
        format!(r#"{{"jsonrpc":"2.0","id":2,"method":"resources/read","params":{{"uri":"ui://claude-statusline/footer","_meta":{{"com.openai/codex-statusline-state":{snapshot}}}}}}}"#),
        "\u{fffd}not json".to_string(),
    ];
    let mut child = Command::new(env!("CARGO_BIN_EXE_claude-statusline"))
        .args(["codex", "--profile", "e2e"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn");
    let mut stdin = child.stdin.take().unwrap();
    for line in &session {
        stdin.write_all(line.as_bytes()).unwrap();
        stdin.write_all(b"\n").unwrap();
    }
    drop(stdin);
    let out = child.wait_with_output().expect("wait");
    assert!(out.status.success(), "non-zero exit");
    let stdout = String::from_utf8(out.stdout).unwrap();
    let replies: Vec<&str> = stdout.lines().collect();

    struct Case {
        description: &'static str,
        reply: usize,
        expected: &'static [&'static str],
    }
    let cases = [
        Case { description: "initialize", reply: 0, expected: &[r#""id":0,"result":{"protocolVersion""#] },
        Case { description: "tools/list", reply: 1, expected: &[r#""id":1"#, "ui://claude-statusline/footer"] },
        Case { description: "resources/read with --profile", reply: 2, expected: &[r#""id":2"#, r#"\"e2e\""#, "GPT-5.5"] },
        Case { description: "garbage gets a parse error", reply: 3, expected: &[r#""id":null,"error":{"code":-32700"#] },
    ];
    assert_eq!(replies.len(), cases.len(), "reply count: {stdout}");
    for c in cases {
        for want in c.expected {
            assert!(replies[c.reply].contains(want), "{}: {want:?} not in {}", c.description, replies[c.reply]);
        }
    }
}
