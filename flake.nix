{
  description = "Throwaway GPUI + libghostty-vt foundation spike";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs = { self, nixpkgs }:
    let
      system = "x86_64-linux";
      pkgs = import nixpkgs { inherit system; };
    in {
      devShells.${system}.default = pkgs.mkShell {
        packages = with pkgs; [
          cargo
          clang
          fontconfig
          freetype
          git
          libxkbcommon
          mesa
          pkg-config
          rustc
          wayland
          vulkan-loader
          xorg.libX11
          xorg.libXcursor
          xorg.libXi
          xorg.libxcb
          zig
        ];

        LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath (with pkgs; [
          fontconfig
          freetype
          libxkbcommon
          mesa
          wayland
          vulkan-loader
          xorg.libX11
          xorg.libXcursor
          xorg.libXi
          xorg.libxcb
        ]);
      };
    };
}
