# claude-statusline

One status line for both [Claude Code](https://code.claude.com/docs/en/statusline)
and the [Codex](https://github.com/openai/codex) TUI, showing what you need to
know while an AI agent works in your repo:

```
personal nixos main | c:12k:42%/57% | s:30%/70% 2h13m | Opus 5.5 hi | idle 15m
```

## Why

Both tools hide the numbers that decide how a session goes. You find out the
context window is full when quality drops or it compacts, and you find out
you've hit a rate limit when it stops. This line keeps them in view:

- **Which account, which repo, which branch:** with several profiles (work,
  personal, client logins) and worktrees, it's easy to run the agent in the
  wrong one. The profile comes first.
- **Context use, in tokens and %:** turns orange at 50% and red at 80%, or at
  180k tokens regardless of window size, before long-context pricing and
  quality loss set in.
- **Rate-limit use and when it resets:** the 5-hour window, and the weekly
  one once it's the one that matters, so you can pace work or switch accounts.
- **Model and effort:** what you're actually running, since both are easy to
  change mid-session.
- **Idle time:** how long since anything happened, so an agent that's been
  waiting on you for 40 minutes is obvious when you come back to the window.

The two tools use the same layout and colours, so moving between them doesn't
mean learning a second line.

It is built to cost nothing. Claude Code runs the statusline as a new process
on every update, so it is a static, std-only Rust binary that runs in about
0.4 ms (most of it process start-up; rendering is ~3 µs), with no `jq`, shell
or `git` subprocess. Codex keeps it running as a small MCP server instead,
where each read takes ~6 µs and memory stays flat. All text from outside the
program (JSON, git HEAD files, credentials) is stripped of control
characters, so a malicious repo can't inject terminal escape sequences.

## Install

With Nix, see [Use with Nix](#use-with-nix). Otherwise:

```
cargo build --release   # target/release/claude-statusline
```

Claude Code, in `~/.claude/settings.json` (or your profile's
`$CLAUDE_CONFIG_DIR/settings.json`):

```json
"statusLine": {
  "type": "command",
  "command": "/path/to/claude-statusline",
  "refreshInterval": 60
}
```

Codex: see [Codex](#codex).

## Segments

| Segment | Meaning |
|---|---|
| `personal` | profile: basename of `$CLAUDE_CONFIG_DIR` (`default` if unset) |
| `nixos main` | project directory and git branch (or short hash when detached), from the nearest `.git` above it, worktrees included |
| `c:12k:42%/57%` | context window: tokens in context, % used / % remaining. Green <50%, orange <80%, red ≥80% or ≥180k tokens |
| `s:30%/70% 2h13m` | 5-hour rate limit: % used / % remaining, time to reset. Falls back to the subscription tier when Claude Code doesn't send rate limits |
| `w:63%/37%` | 7-day rate limit, same palette; shown from 50% used, or always (with its reset, e.g. `3d16h`) when there's no 5-hour window |
| `Opus 5.5 hi` | model and effort level |
| `idle 15m` | time since the transcript last changed (your prompt or Claude's output), in 5-minute steps, pastel orange. Hidden for the first 5 minutes |

## Codex

`claude-statusline codex` is a stdio MCP statusline provider for the Codex
TUI (0.171+, experimental). Codex starts it once and reads it every second,
so there's no per-refresh start-up cost: a read takes ~6 µs. It shows the same
segments, translated from Codex's snapshot:

| Segment | From |
|---|---|
| profile | `--profile NAME`, else basename of `$CODEX_HOME` (only visible if listed in the server's `env_vars`), else Codex's `config.profile` |
| dir, branch | `thread.workspace.cwd`; branch from the nearest `.git` above it (worktrees too) |
| `c:` | `thread.usage.context.used_tokens` / `used_percentage` |
| `s:`, `w:` | `account.rate_limits[limit_id = "codex"]`'s `primary_limit` and `secondary_limit`, sorted by `window_minutes`: up to a day is `s:`, longer is `w:` (on some plans the primary window is the weekly one). Remaining clamps at 0 |
| model, effort | `thread.model.display_name`, `reasoning_level` |
| `idle` | time since the thread's agent state or token counts last changed; hidden while the agent is working |

Enable it in `$CODEX_HOME/config.toml`:

```toml
[features]
tui_extensions_statusline = true

[tui]
status_line_provider = { server = "claude-statusline", tool = "statusline" }

[mcp_servers.claude-statusline]
command = "/path/to/claude-statusline"
args = ["codex", "--profile", "personal"]
```

Or pick it in `/statusline` → *Choose a status line provider*. If the footer
falls back to the default, F2 shows why. Bad input renders defaults rather
than an error, since any failed read stops Codex polling.

## Security

Everything it prints comes from outside the program: Claude Code's JSON or
Codex's snapshot, the environment, the `.git/HEAD` of whatever directory
you're in, and the credentials file. Control characters are stripped from all
of them, so a crafted repository can't inject terminal escape sequences
(clipboard writes via OSC 52, fake hyperlinks, title changes). Branch names are
limited to git-ref-safe ASCII and capped in length.

The credentials file is only read for its `subscriptionType` field; no token
is printed or stored.

## Use with Nix

Pin a tag; `flake.lock` records the commit and narHash:

```nix
# flake.nix
inputs.claude-statusline = {
  url = "github:randomizedcoder/claude-statusline/v0.5.1";
  inputs.nixpkgs.follows = "nixpkgs";
};
```

Then point Claude Code's `settings.json` at the binary, e.g. from home-manager:

```nix
statusLine = {
  type = "command";
  command = lib.getExe inputs.claude-statusline.packages.${pkgs.stdenv.hostPlatform.system}.default;
  # Re-run every 60s as well as on events, so `idle` advances while you're away.
  refreshInterval = 60;
};
```

Or use `overlays.default`, which adds `pkgs.claude-statusline`.

## Performance

Claude Code runs the statusline on every update, so per-run cost matters.
Rendering takes ~3 µs; nearly all the rest is process start-up, which is what
v0.3.0 cuts:

- **Static musl binary on Linux:** no dynamic loader, no shared libraries to
  map and relocate.
- **`#![no_main]`:** skips std's runtime init (reading `/proc/self/maps`,
  SIGSEGV/SIGBUS handlers on an alternate stack, polling fds 0-2).
- **One stdin read** into a pre-sized buffer, instead of 32/32/64/128/... byte
  probe reads.
- **Zero-copy JSON strings:** they borrow from the input unless escaped, which
  halved render time.

Measured on a Threadripper PRO 3945WX, realistic 1 KiB input:

| | v0.2.0 | v0.3.0 |
|---|---|---|
| wall time per run (hyperfine, 10k runs) | 915 µs | 444 µs |
| user-space instructions | 378k | 53k |
| page faults | 78 | 25 |
| syscalls | 94 | 27 |
| render only, in-process | 6.8 µs | 3.4 µs |

Reproduce, optionally comparing against other builds:

```
nix run .#bench -- /nix/store/...-claude-statusline-0.2.0/bin/claude-statusline
cargo test --release -- --ignored --nocapture bench_   # render + Codex read
```

## Develop

```
nix develop
cargo test
nix flake check   # build + tests + clippy -D warnings
```

Tests are table-driven (`src/tests.rs`); each row has a `description` and
`expected`. `tests/binary.rs` runs the built binary end to end, in both
modes. Layout: `src/json.rs` (parser/writer), `src/git.rs` (HEAD lookup), `src/status.rs` (shared
`Status` → spans → ANSI or UiTree), `src/claude.rs` and `src/codex.rs`
(front-ends).
