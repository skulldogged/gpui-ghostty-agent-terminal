{
  description = "Cross-platform GPUI terminal powered by libghostty-vt";

  inputs.ghostty = {
    url = "github:ghostty-org/ghostty/4c725242b7dbe8c77c6e227ef1f9540c5ef17921";
    flake = false;
  };
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    {
      self,
      ghostty,
      nixpkgs,
    }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      forAllSystems = nixpkgs.lib.genAttrs systems;
      ghosttyRevision = "4c725242b7dbe8c77c6e227ef1f9540c5ef17921";

      runtimeLibraries = pkgs: with pkgs; [
        fontconfig
        freetype
        libx11
        libxcb
        libxcursor
        libxi
        libxkbcommon
        mesa
        vulkan-loader
        wayland
      ];

      packageFor = system:
        let
          pkgs = import nixpkgs { inherit system; };
          libghosttyVt = pkgs.callPackage "${ghostty}/nix/libghostty-vt.nix" {
            revision = ghosttyRevision;
            optimize = "ReleaseFast";
          };
          libraries = runtimeLibraries pkgs;
        in
        pkgs.rustPlatform.buildRustPackage {
          pname = "agent-terminal";
          version = "0.0.0";

          src = self;
          cargoLock = {
            lockFile = ./Cargo.lock;
            allowBuiltinFetchGit = true;
          };

          GHOSTTY_VT_INCLUDE_DIR = "${libghosttyVt.dev}/include";
          GHOSTTY_VT_LIB_DIR = "${libghosttyVt.dev}/lib";

          nativeBuildInputs = with pkgs; [
            copyDesktopItems
            makeWrapper
            pkg-config
          ];
          buildInputs = libraries;

          desktopItems = [
            (pkgs.makeDesktopItem {
              name = "agent-terminal";
              desktopName = "Agent Terminal";
              comment = "Graphical terminal multiplexer powered by libghostty-vt";
              exec = "agent-terminal";
              icon = "utilities-terminal";
              categories = [
                "System"
                "TerminalEmulator"
              ];
              terminal = false;
              startupNotify = true;
            })
          ];

          postFixup = ''
            wrapProgram "$out/bin/agent-terminal" \
              --prefix LD_LIBRARY_PATH : ${pkgs.lib.makeLibraryPath libraries}
          '';

          meta = {
            description = "Cross-platform GPUI terminal powered by libghostty-vt";
            homepage = "https://github.com/skulldogged/gpui-ghostty-agent-terminal";
            mainProgram = "agent-terminal";
            platforms = systems;
          };
        };
    in
    {
      packages = forAllSystems (system: rec {
        agent-terminal = packageFor system;
        default = agent-terminal;
      });

      devShells = forAllSystems (
        system:
        let
          pkgs = import nixpkgs { inherit system; };
          libraries = runtimeLibraries pkgs;
        in
        {
          default = pkgs.mkShell {
            packages = with pkgs; [
              cargo
              clang
              clippy
              git
              pkg-config
              rustc
              rustfmt
              zig
            ] ++ libraries;

            LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath libraries;
          };
        }
      );
    };
}
