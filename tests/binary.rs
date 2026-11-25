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
