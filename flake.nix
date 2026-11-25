{
  description = "Fast, dependency-free statusline for Claude Code";

  inputs.nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";

  outputs = { self, nixpkgs }:
    let
      lib = nixpkgs.lib;
      systems = [ "x86_64-linux" "aarch64-linux" "aarch64-darwin" "x86_64-darwin" ];
      forAll = f: lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});

      # Only what the build needs, so README/CI edits don't change the hash.
      src = lib.fileset.toSource {
        root = ./.;
        fileset = lib.fileset.unions [ ./Cargo.toml ./Cargo.lock ./src ./tests ];
      };

      mkPackageWith = pkgs: pkgs.rustPlatform.buildRustPackage {
        pname = "claude-statusline";
        version = (lib.importTOML ./Cargo.toml).package.version;
        inherit src;
        cargoLock.lockFile = ./Cargo.lock;
        # cargo test (the table-driven suite) runs in checkPhase.
        doCheck = true;
        meta = {
          description = "Fast, dependency-free statusline for Claude Code";
          homepage = "https://github.com/randomizedcoder/claude-statusline";
          license = lib.licenses.mit;
          mainProgram = "claude-statusline";
          platforms = lib.platforms.unix;
        };
      };

      # On Linux, link statically against musl: no dynamic loader, no shared
      # libs to map and relocate. With no_main this is ~2.2x faster per run than
      # the dynamic glibc build (see `nix run .#bench`). macOS can't be static.
      mkPackage = pkgs:
        mkPackageWith (if pkgs.stdenv.hostPlatform.isLinux then pkgs.pkgsStatic else pkgs);

      # Startup + render benchmark. Extra args are other binaries to compare
      # against, e.g. an older /nix/store/...-claude-statusline-0.2.0/bin/...
      mkBench = pkgs: pkg: pkgs.writeShellApplication {
        name = "claude-statusline-bench";
        runtimeInputs = [ pkgs.hyperfine ]
          ++ lib.optionals pkgs.stdenv.hostPlatform.isLinux [ pkgs.perf pkgs.strace ];
        text = ''
          fixture=${src}/tests/fixtures/claude.json
          bins=("${lib.getExe pkg}" "$@")
          runs=''${RUNS:-5000}

          echo "== wall time per run, realistic input (hyperfine, $runs runs)"
          hyperfine -N --warmup 500 --runs "$runs" --input "$fixture" "''${bins[@]}"

          if command -v perf >/dev/null; then
            for b in "''${bins[@]}"; do
              echo "== per-exec counters, empty stdin: $b"
              perf stat -r 2000 -e task-clock,page-faults,instructions:u,instructions:k \
                "$b" </dev/null >/dev/null ||
                echo "(perf stat failed: check kernel.perf_event_paranoid)"
            done
            for b in "''${bins[@]}"; do
              echo "== syscalls: $b"
              strace -f -c "$b" <"$fixture" 2>&1 >/dev/null | tail -1
            done
          fi
          echo "== in-process render only: cargo test --release -- --ignored --nocapture bench_render"
        '';
      };
    in
    {
      packages = forAll (pkgs: rec {
        claude-statusline = mkPackage pkgs;
        default = claude-statusline;
      });

      apps = forAll (pkgs: {
        bench = {
          type = "app";
          program = lib.getExe
            (mkBench pkgs self.packages.${pkgs.stdenv.hostPlatform.system}.default);
        };
      });

      overlays.default = final: _prev: {
        claude-statusline = mkPackage final;
      };

      checks = forAll (pkgs: {
        build = self.packages.${pkgs.stdenv.hostPlatform.system}.default;
        clippy = (mkPackageWith pkgs).overrideAttrs (old: {
          pname = "claude-statusline-clippy";
          nativeBuildInputs = old.nativeBuildInputs ++ [ pkgs.clippy ];
          buildPhase = ''
            runHook preBuild
            cargo clippy --offline --all-targets -- -D warnings
            runHook postBuild
          '';
          doCheck = false;
          installPhase = "touch $out";
        });
      });

      devShells = forAll (pkgs: {
        default = pkgs.mkShell {
          packages = with pkgs; [ cargo rustc clippy rustfmt rust-analyzer hyperfine ]
            ++ lib.optionals pkgs.stdenv.hostPlatform.isLinux [ perf strace ];
        };
      });
    };
}
