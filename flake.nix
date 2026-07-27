{
  description = "Git worktree manager driven by a per-project wt.toml";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  # Prebuilt binaries instead of a local compilation. Nix only applies a flake's
  # nixConfig after an interactive confirmation, and ignores it outright in scripts and
  # CI: this is a convenience, not the supported path. The reliable way is the
  # `nix.settings` snippet in the README, or `cachix use catvert`.
  nixConfig = {
    extra-substituters = [ "https://catvert.cachix.org" ];
    extra-trusted-public-keys = [
      "catvert.cachix.org-1:R5plivdLnx2WtmZkBryZwUF51Uvl6TJldhFGYOcyPXg="
    ];
  };

  outputs = { self, nixpkgs }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" "x86_64-darwin" "aarch64-darwin" ];
      forAllSystems = f:
        nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});

      # Taking the version from Cargo.toml keeps a single source of truth: bumping the
      # crate is enough, the flake follows.
      cargoToml = builtins.fromTOML (builtins.readFile ./Cargo.toml);

      mkWt = pkgs:
        pkgs.rustPlatform.buildRustPackage {
          pname = "wt";
          inherit (cargoToml.package) version;
          src = ./.;

          # Reading Cargo.lock avoids the vendor hash that would need updating on every
          # dependency bump.
          cargoLock.lockFile = ./Cargo.lock;

          nativeBuildInputs = [ pkgs.installShellFiles ];

          # The integration tests drive real repositories, and the sandbox has no git.
          nativeCheckInputs = [ pkgs.git ];

          # Completions come from the binary itself, so they can never drift from the
          # actual commands. What gets installed is the script that asks `wt` for the
          # candidates on every TAB — safe to write to a file here, where the script and
          # the binary it calls come out of the same build.
          # Skipped when cross-compiling, where the binary cannot be run.
          postInstall = pkgs.lib.optionalString
            (pkgs.stdenv.buildPlatform.canExecute pkgs.stdenv.hostPlatform) ''
              installShellCompletion --cmd wt \
                --bash <($out/bin/wt completions bash) \
                --zsh <($out/bin/wt completions zsh) \
                --fish <($out/bin/wt completions fish)
            '';

          meta = with pkgs.lib; {
            inherit (cargoToml.package) description;
            homepage = "https://github.com/Catvert/wt";
            license = licenses.mit;
            mainProgram = "wt";
            platforms = platforms.unix;
          };
        };
    in
    {
      packages = forAllSystems (pkgs: rec {
        wt = mkWt pkgs;
        default = wt;
      });

      overlays.default = final: _prev: { wt = mkWt final; };

      devShells = forAllSystems (pkgs: {
        default = pkgs.mkShell {
          packages = with pkgs; [ cargo rustc clippy rustfmt rust-analyzer git ];
          RUST_SRC_PATH = "${pkgs.rustPlatform.rustLibSrc}";
        };
      });

      formatter = forAllSystems (pkgs: pkgs.nixpkgs-fmt);
    };
}
