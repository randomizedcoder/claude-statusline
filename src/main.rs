// claude-statusline
//
// A fast statusline for Claude Code and Codex, std only (no dependencies).
//
//   claude-statusline                    Claude Code: statusline JSON on stdin,
//                                        one ANSI-coloured line on stdout
//   claude-statusline codex [--profile NAME]
//                                        Codex: a stdio MCP statusline provider
//                                        (see codex.rs)
//
// Layout: json.rs (parser/writer), status.rs (shared Status -> spans -> ANSI
// or UiTree), claude.rs and codex.rs (front-ends: input -> Status).
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

// Startup cost dominates (the render itself is ~3µs), so skip std's runtime
// init: `fn main` would first read /proc/self/maps, install SIGSEGV/SIGBUS
// handlers on an alt stack, poll fds 0-2 and ignore SIGPIPE. None of that is
// needed here; a closed stdout just ends the process.
#![cfg_attr(not(test), no_main)]
// The run()s and real fs readers are only reached from the no_mangle main.
#![cfg_attr(test, allow(dead_code))]

mod claude;
mod codex;
mod json;
mod status;

#[cfg(not(test))]
use std::ffi::{c_char, CStr};

#[cfg(not(test))]
#[no_mangle]
/// The process entry point (see no_main above).
///
/// # Safety
///
/// Only the C runtime calls this, with argc valid NUL-terminated argv strings.
pub unsafe extern "C" fn main(argc: i32, argv: *const *const c_char) -> i32 {
    let args: Vec<&str> = (0..argc.max(0) as usize)
        // SAFETY: the C runtime passes argc valid NUL-terminated strings,
        // alive for the whole process.
        .filter_map(|i| unsafe { CStr::from_ptr(*argv.add(i)) }.to_str().ok())
        .collect();
    dispatch(&args);
    0
}

fn dispatch(args: &[&str]) {
    match args.get(1) {
        Some(&"codex") => codex::run(codex::profile(&args[2..])),
        _ => claude::run(),
    }
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_codex;
