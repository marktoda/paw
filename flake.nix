{
  description = "paw: a pass client for iPhone and iPad with YubiKey support";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs = {nixpkgs, ...}: let
    systems = ["x86_64-linux" "aarch64-linux" "aarch64-darwin"];
    forAll = f: nixpkgs.lib.genAttrs systems (s: f nixpkgs.legacyPackages.${s});
  in {
    devShells = forAll (pkgs: {
      default = pkgs.mkShell {
        # rustup rather than nixpkgs rustc: the iOS targets come from rustup.
        packages = [
          pkgs.rustup
          pkgs.gcc
          pkgs.pkg-config
          pkgs.gnupg
          # The sync tests run git-upload-pack and git-receive-pack behind an
          # in-process SSH server.
          pkgs.git
          # Signing and installing device builds from Linux.
          pkgs.rcodesign
          pkgs.openssl
          pkgs.unzip
          pkgs.uv
        ];
      };
    });
  };
}
