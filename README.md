# claude-statusline

A fast, dependency-free [statusline](https://code.claude.com/docs/en/statusline)
for Claude Code. Single Rust binary, std only, sub-millisecond.

```
personal nixos main | c:12k:42%/57% | s:30%/70% 2h13m | Opus 5.5 hi | idle 15m
```

| Segment | Meaning |
|---|---|
| `personal` | profile: basename of `$CLAUDE_CONFIG_DIR` (`default` if unset) |
| `nixos main` | project directory and git branch (or short hash when detached) |
| `c:12k:42%/57%` | context window: tokens in context, % used / % remaining. Green <50%, orange <80%, red ≥80% or ≥180k tokens |
| `s:30%/70% 2h13m` | 5-hour rate limit: % used / % remaining, time to reset. Falls back to the subscription tier when Claude Code doesn't send rate limits |
| `Opus 5.5 hi` | model and effort level |
| `idle 15m` | time since the transcript last changed (your prompt or Claude's output), in 5-minute steps, pastel orange. Hidden for the first 5 minutes |

## Security

Everything it prints comes from outside the program: Claude Code's JSON,
the environment, the `.git/HEAD` of whatever directory you're in, and the
credentials file. Control characters are stripped from all of them, so a
crafted repository can't inject terminal escape sequences (clipboard writes
via OSC 52, fake hyperlinks, title changes). Branch names are limited to
git-ref-safe ASCII and capped in length.

The credentials file is only read for its `subscriptionType` field; no token
is printed or stored.

## Use with Nix

Pin a tag; `flake.lock` records the commit and narHash:

```nix
# flake.nix
inputs.claude-statusline = {
  url = "github:randomizedcoder/claude-statusline/v0.2.0";
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

## Develop

```
nix develop
cargo test
nix flake check   # build + tests + clippy -D warnings
```

Tests are table-driven (`src/tests.rs`); each row has a `description` and
`expected`.
