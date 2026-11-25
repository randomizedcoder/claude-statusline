use super::*;

// Strip ANSI SGR colour codes (ESC [ ... m) so expectations stay readable.
fn plain(s: &str) -> String {
    let mut out = String::new();
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\x1b' && it.peek() == Some(&'[') {
            for d in it.by_ref() {
                if d == 'm' {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

// The only escapes render() may emit are its own SGR colour codes.
fn only_sgr_escapes(s: &str) -> bool {
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == 0x1b {
            if b.get(i + 1) != Some(&b'[') {
                return false;
            }
            i += 2;
            while i < b.len() && (b[i].is_ascii_digit() || b[i] == b';') {
                i += 1;
            }
            if b.get(i) != Some(&b'm') {
                return false;
            }
        } else if (b[i] < 0x20 && b[i] != b'\t') || b[i] == 0x7f {
            return false;
        }
        i += 1;
    }
    !s.chars().any(|c| ('\u{80}'..='\u{9f}').contains(&c))
}

fn no_head(_: &str) -> Option<String> {
    None
}
fn no_tier(_: Option<&str>) -> Option<String> {
    None
}
fn head_main(_: &str) -> Option<String> {
    Some("ref: refs/heads/main\n".into())
}
fn head_evil(_: &str) -> Option<String> {
    Some("ref: refs/heads/a\x1b]52;c;ZXZpbA==\x07b\n".into())
}
fn tier_max(_: Option<&str>) -> Option<String> {
    Some("max".into())
}
fn tier_evil(_: Option<&str>) -> Option<String> {
    Some("\x1b]0;pwned\x07pro".into())
}

fn no_mtime(_: &str) -> Option<i64> {
    None
}
fn mtime_12m_ago(_: &str) -> Option<i64> {
    Some(NOW - 12 * 60)
}
fn mtime_1m_ago(_: &str) -> Option<i64> {
    Some(NOW - 60)
}

const NOW: i64 = 1_800_000_000;

fn env(
    config_dir: Option<&str>,
    read_head: fn(&str) -> Option<String>,
    read_tier: fn(Option<&str>) -> Option<String>,
) -> Env {
    Env {
        config_dir: config_dir.map(str::to_string),
        now: NOW,
        read_head,
        read_tier,
        read_mtime: no_mtime,
    }
}

#[test]
fn test_parse_string() {
    struct Case {
        description: &'static str,
        input: &'static str,
        expected: Option<&'static str>, // value of key "k"
        borrowed: bool,                 // zero-copy expected
    }
    let cases = [
        Case { description: "plain string borrows", input: r#"{"k":"abc"}"#, expected: Some("abc"), borrowed: true },
        Case { description: "empty string borrows", input: r#"{"k":""}"#, expected: Some(""), borrowed: true },
        Case { description: "multibyte UTF-8 borrows", input: r#"{"k":"café-日本"}"#, expected: Some("café-日本"), borrowed: true },
        Case { description: "escaped quote copies", input: r#"{"k":"a\"b"}"#, expected: Some("a\"b"), borrowed: false },
        Case { description: "escape after multibyte", input: r#"{"k":"é\nx"}"#, expected: Some("é\nx"), borrowed: false },
        Case { description: "unicode escape", input: r#"{"k":"\u00e9!"}"#, expected: Some("é!"), borrowed: false },
        Case { description: "escaped key matches", input: r#"{"\u006b":"v"}"#, expected: Some("v"), borrowed: true },
        Case { description: "unterminated string fails", input: r#"{"k":"abc"#, expected: None, borrowed: false },
        Case { description: "unterminated after escape fails", input: r#"{"k":"a\n"#, expected: None, borrowed: false },
        Case { description: "bad escape fails", input: r#"{"k":"a\qb"}"#, expected: None, borrowed: false },
        Case { description: "short unicode escape fails", input: r#"{"k":"\u12"}"#, expected: None, borrowed: false },
        Case { description: "non-string value", input: r#"{"k":1}"#, expected: None, borrowed: false },
    ];
    for c in cases {
        let v = parse(c.input);
        let got = v.as_ref().and_then(|v| v.get("k"));
        assert_eq!(got.and_then(|v| v.as_str()), c.expected, "{}", c.description);
        if c.expected.is_some() {
            let b = matches!(got, Some(Val::Str(Cow::Borrowed(_))));
            assert_eq!(b, c.borrowed, "{}: borrowed", c.description);
        }
    }
}

#[test]
fn test_safe() {
    struct Case {
        description: &'static str,
        input: &'static str,
        max: usize,
        expected: &'static str,
    }
    let cases = [
        Case { description: "plain text unchanged", input: "hello", max: 10, expected: "hello" },
        Case { description: "empty", input: "", max: 10, expected: "" },
        Case { description: "ESC CSI stripped", input: "a\x1b[31mb", max: 10, expected: "a[31mb" },
        Case { description: "OSC + BEL stripped", input: "a\x1b]0;t\x07b", max: 20, expected: "a]0;tb" },
        Case { description: "C1 CSI (U+009B) stripped", input: "a\u{9b}31mb", max: 10, expected: "a31mb" },
        Case { description: "DEL stripped", input: "a\x7fb", max: 10, expected: "ab" },
        Case { description: "newline and tab stripped", input: "a\nb\tc", max: 10, expected: "abc" },
        Case { description: "unicode kept", input: "café-日本", max: 10, expected: "café-日本" },
        Case { description: "capped at max chars", input: "abcdef", max: 3, expected: "abc" },
        Case { description: "max 0", input: "abc", max: 0, expected: "" },
        Case { description: "cap counts chars, not bytes", input: "ééé", max: 2, expected: "éé" },
    ];
    for c in cases {
        assert_eq!(safe(c.input, c.max), c.expected, "{}", c.description);
    }
}

#[test]
fn test_branch_from_head() {
    let long_in = format!("ref: refs/heads/{}\n", "x".repeat(100));
    let long_out = "x".repeat(MAX_BRANCH);
    struct Case<'a> {
        description: &'a str,
        head: &'a str,
        expected: Option<&'a str>,
    }
    let cases = [
        Case { description: "simple branch", head: "ref: refs/heads/main\n", expected: Some("main") },
        Case { description: "branch with slash, dot, dash", head: "ref: refs/heads/feat/x-1.2\n", expected: Some("feat/x-1.2") },
        Case { description: "branch with + and @", head: "ref: refs/heads/a+b@c\n", expected: Some("a+b@c") },
        Case { description: "detached hash shortened to 7", head: "0123456789abcdef0123\n", expected: Some("0123456") },
        Case { description: "short detached value kept", head: "abc\n", expected: Some("abc") },
        Case { description: "OSC 52 clipboard injection neutralised", head: "ref: refs/heads/a\x1b]52;c;ZXZpbA==\x07b\n", expected: Some("a52cZXZpbAb") },
        Case { description: "CSI injection neutralised", head: "ref: refs/heads/\x1b[2Jmain", expected: Some("2Jmain") },
        Case { description: "multi-byte detached value: no panic, nothing left", head: "ééééééééé\n", expected: None },
        Case { description: "multi-byte in branch dropped", head: "ref: refs/heads/café\n", expected: Some("caf") },
        Case { description: "empty file", head: "", expected: None },
        Case { description: "whitespace only", head: " \n\t", expected: None },
        Case { description: "ref prefix with empty name", head: "ref: refs/heads/\n", expected: None },
        Case { description: "branch capped at MAX_BRANCH", head: &long_in, expected: Some(&long_out) },
        Case { description: "non-heads ref treated as detached (7 chars)", head: "ref: refs/tags/v1", expected: Some("refrefs") },
    ];
    for c in cases {
        assert_eq!(branch_from_head(c.head).as_deref(), c.expected, "{}", c.description);
    }
}

#[test]
fn test_fmt_reset() {
    struct Case {
        description: &'static str,
        resets_at: i64,
        expected: Option<&'static str>,
    }
    let cases = [
        Case { description: "in the past", resets_at: NOW - 10, expected: None },
        Case { description: "exactly now", resets_at: NOW, expected: None },
        Case { description: "under a minute", resets_at: NOW + 59, expected: Some("0m") },
        Case { description: "45 minutes", resets_at: NOW + 45 * 60, expected: Some("45m") },
        Case { description: "59m59s", resets_at: NOW + 3599, expected: Some("59m") },
        Case { description: "exactly one hour", resets_at: NOW + 3600, expected: Some("1h0m") },
        Case { description: "2h13m", resets_at: NOW + 2 * 3600 + 13 * 60, expected: Some("2h13m") },
    ];
    for c in cases {
        assert_eq!(fmt_reset(c.resets_at, NOW).as_deref(), c.expected, "{}", c.description);
    }
}

#[test]
fn test_fmt_idle() {
    struct Case {
        description: &'static str,
        idle_secs: i64,
        expected: Option<&'static str>,
    }
    let cases = [
        Case { description: "just changed", idle_secs: 0, expected: None },
        Case { description: "mtime in the future (clock skew)", idle_secs: -600, expected: None },
        Case { description: "4m59s: still hidden", idle_secs: 299, expected: None },
        Case { description: "exactly 5m: first step shown", idle_secs: 300, expected: Some("5m") },
        Case { description: "9m59s rounds down to 5m", idle_secs: 599, expected: Some("5m") },
        Case { description: "10m", idle_secs: 600, expected: Some("10m") },
        Case { description: "59m rounds down to 55m", idle_secs: 59 * 60, expected: Some("55m") },
        Case { description: "exactly one hour", idle_secs: 3600, expected: Some("1h0m") },
        Case { description: "2h17m rounds down to 2h15m", idle_secs: 2 * 3600 + 17 * 60, expected: Some("2h15m") },
    ];
    for c in cases {
        assert_eq!(fmt_idle(NOW - c.idle_secs, NOW).as_deref(), c.expected, "{}", c.description);
    }
}

#[test]
fn test_abbrevs_and_pick() {
    struct Case {
        description: &'static str,
        got: String,
        expected: &'static str,
    }
    let cases = [
        Case { description: "tier enterprise", got: tier_abbrev("enterprise"), expected: "ent" },
        Case { description: "tier max", got: tier_abbrev("max"), expected: "max" },
        Case { description: "tier unknown truncated + lowercased", got: tier_abbrev("SuperPlan"), expected: "supe" },
        Case { description: "tier empty", got: tier_abbrev(""), expected: "" },
        Case { description: "effort medium", got: effort_abbrev("medium").into(), expected: "med" },
        Case { description: "effort xhigh", got: effort_abbrev("xhigh").into(), expected: "xhi" },
        Case { description: "effort unknown passthrough", got: effort_abbrev("turbo").into(), expected: "turbo" },
        Case { description: "pick 49 -> low", got: pick(49, "L", "M", "H").into(), expected: "L" },
        Case { description: "pick 50 -> mid (boundary)", got: pick(50, "L", "M", "H").into(), expected: "M" },
        Case { description: "pick 79 -> mid", got: pick(79, "L", "M", "H").into(), expected: "M" },
        Case { description: "pick 80 -> high (boundary)", got: pick(80, "L", "M", "H").into(), expected: "H" },
        Case { description: "pick negative -> low", got: pick(-5, "L", "M", "H").into(), expected: "L" },
        Case { description: "profile from config dir", got: profile_name(Some("/h/.claude/profiles/personal")), expected: "personal" },
        Case { description: "profile with trailing slash", got: profile_name(Some("/h/.claude/profiles/runpod/")), expected: "runpod" },
        Case { description: "profile unset", got: profile_name(None), expected: "default" },
        Case { description: "profile empty", got: profile_name(Some("")), expected: "default" },
    ];
    for c in cases {
        assert_eq!(c.got, c.expected, "{}", c.description);
    }
}

#[test]
fn test_render() {
    struct Case {
        description: &'static str,
        input: &'static str,
        env: Env,
        expected: &'static str,
    }
    let cases = [
        Case {
            description: "empty input: defaults",
            input: "",
            env: env(None, no_head, no_tier),
            expected: "default unknown | c:0k:0%/100%",
        },
        Case {
            description: "malformed JSON: defaults",
            input: "{not json",
            env: env(None, no_head, no_tier),
            expected: "default unknown | c:0k:0%/100%",
        },
        Case {
            description: "full line with branch, session, reset, model, effort",
            input: r#"{"workspace":{"project_dir":"/home/das/nixos"},
                       "context_window":{"used_percentage":42.7,"remaining_percentage":57.3,
                                         "total_input_tokens":12000,"total_output_tokens":400},
                       "rate_limits":{"five_hour":{"used_percentage":30,"resets_at":1800007980}},
                       "model":{"display_name":"Opus 5.5","id":"claude-opus-5-5"},
                       "effort":{"level":"high"}}"#,
            env: env(Some("/home/das/.claude/profiles/personal"), head_main, no_tier),
            expected: "personal nixos main | c:12k:42%/57% | s:30%/70% 2h13m | Opus 5.5 hi",
        },
        Case {
            description: "model id fallback when display_name absent",
            input: r#"{"model":{"id":"claude-opus-5-5"}}"#,
            env: env(None, no_head, no_tier),
            expected: "default unknown | c:0k:0%/100% | claude-opus-5-5",
        },
        Case {
            description: "tier shown when rate_limits absent",
            input: r#"{"workspace":{"project_dir":"/x/repo"}}"#,
            env: env(None, no_head, tier_max),
            expected: "default repo | c:0k:0%/100% | max",
        },
        Case {
            description: "token rounding: 499 -> 0k, 500 -> 1k boundary",
            input: r#"{"context_window":{"total_input_tokens":500}}"#,
            env: env(None, no_head, no_tier),
            expected: "default unknown | c:1k:0%/100%",
        },
        Case {
            description: "expired reset time omitted",
            input: r#"{"rate_limits":{"five_hour":{"used_percentage":90,"resets_at":1}}}"#,
            env: env(None, no_head, no_tier),
            expected: "default unknown | c:0k:0%/100% | s:90%/10%",
        },
        Case {
            description: "idle segment after model + effort",
            input: r#"{"transcript_path":"/t.jsonl","model":{"display_name":"Opus 5.5"},"effort":{"level":"high"}}"#,
            env: Env { read_mtime: mtime_12m_ago, ..env(None, no_head, no_tier) },
            expected: "default unknown | c:0k:0%/100% | Opus 5.5 hi | idle 10m",
        },
        Case {
            description: "idle hidden within the first 5 minutes",
            input: r#"{"transcript_path":"/t.jsonl"}"#,
            env: Env { read_mtime: mtime_1m_ago, ..env(None, no_head, no_tier) },
            expected: "default unknown | c:0k:0%/100%",
        },
        Case {
            description: "idle hidden without transcript_path",
            input: r#"{}"#,
            env: Env { read_mtime: mtime_12m_ago, ..env(None, no_head, no_tier) },
            expected: "default unknown | c:0k:0%/100%",
        },
        Case {
            description: "escape injection via branch, dir, model, effort, tier, profile",
            input: "{\"workspace\":{\"project_dir\":\"/x/d\\u001b]0;t\\u0007ir\"},\
                    \"model\":{\"display_name\":\"M\\u001b[2J\\u009b1m\"},\
                    \"effort\":{\"level\":\"\\u001b]52;c;eA==\\u0007\"}}",
            env: env(Some("/p/pro\x1b]8;;http://e\x07file"), head_evil, tier_evil),
            // Every escape introducer is gone; the leftovers are inert text.
            expected: "efile d]0;tir a52cZXZpbAb | c:0k:0%/100% | ]0;pwnedpro | M[2J1m ]52;c;eA==",
        },
    ];
    for c in cases {
        let got = render(c.input, &c.env);
        assert!(only_sgr_escapes(&got), "{}: unexpected control chars in {got:?}", c.description);
        assert_eq!(plain(&got), c.expected, "{}", c.description);
    }
}

// In-process render cost (no exec/startup), on the realistic fixture:
//   cargo test --release -- --ignored --nocapture bench_render
#[test]
#[ignore]
fn bench_render() {
    let input = include_str!("../tests/fixtures/claude.json");
    let e = env(None, no_head, no_tier);
    let n = 200_000;
    let t = std::time::Instant::now();
    let mut bytes = 0;
    for _ in 0..n {
        bytes += std::hint::black_box(render(std::hint::black_box(input), &e)).len();
    }
    let ns = t.elapsed().as_nanos() / n as u128;
    println!("render: {ns} ns/iter ({} B in, {} B out)", input.len(), bytes / n);
}
