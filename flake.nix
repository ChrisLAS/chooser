{
  description = "AirTalk Chooser";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs = { self, nixpkgs, ... }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" ];
      forAllSystems = nixpkgs.lib.genAttrs systems;
    in
    {
      packages = forAllSystems (system:
        let
          pkgs = import nixpkgs { inherit system; };
        in
        {
          default = pkgs.rustPlatform.buildRustPackage {
            pname = "chooser";
            version = "0.1.0";
            src = self;

            cargoLock = {
              lockFile = ./Cargo.lock;
              outputHashes = {
                "tailtalk-0.7.1" = "sha256-ARThxi87cr8P7EdVlfJUMoj5CZze8f0cwLmTNsxdRqw=";
              };
            };

            nativeBuildInputs = [ pkgs.pkg-config ];
            buildInputs = [ pkgs.libpcap ];
            RUSTFLAGS = "-L native=${pkgs.libpcap.lib}/lib";
          };
        });

      apps = forAllSystems (system: {
        default = {
          type = "app";
          program = "${self.packages.${system}.default}/bin/chooser";
        };
      });

      devShells = forAllSystems (system:
        let
          pkgs = import nixpkgs { inherit system; };
        in
        {
          default = pkgs.mkShell {
            packages = [
              pkgs.cargo
              pkgs.libpcap
              pkgs.pkg-config
              pkgs.rustc
              pkgs.rustfmt
            ];

            RUSTFLAGS = "-L native=${pkgs.libpcap.lib}/lib";
          };
        });
    };
}
