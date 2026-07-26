{
  description = "Git worktree manager driven by a per-project wt.toml";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  # Offered to anyone running `nix run github:Catvert/wt`: prebuilt binaries instead of a
  # local compilation. Nix asks for confirmation unless the user is a trusted-user; the
  # permanent way is the `nix.settings` snippet in the README.
  #
  # TODO after creating the cache: replace the key with the one printed by
  #   cachix use wt          (or read it on https://app.cachix.org/cache/wt)
  # An empty list simply means "no cache", so this stays harmless until then.
  nixConfig = {
    extra-substituters = [ "https://wt.cachix.org" ];
    extra-trusted-public-keys = [ ];
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
          # actual commands. Skipped when cross-compiling, where it cannot be run.
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
