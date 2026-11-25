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
        fileset = lib.fileset.unions [ ./Cargo.toml ./Cargo.lock ./src ];
      };

      mkPackage = pkgs: pkgs.rustPlatform.buildRustPackage {
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
    in
    {
      packages = forAll (pkgs: rec {
        claude-statusline = mkPackage pkgs;
        default = claude-statusline;
      });

      overlays.default = final: _prev: {
        claude-statusline = mkPackage final;
      };

      checks = forAll (pkgs: {
        build = self.packages.${pkgs.stdenv.hostPlatform.system}.default;
        clippy = (mkPackage pkgs).overrideAttrs (old: {
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
          packages = with pkgs; [ cargo rustc clippy rustfmt rust-analyzer ];
        };
      });
    };
}
