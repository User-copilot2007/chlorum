{
  description = "Chlorum";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-26.05";
    disko = {
      url = "github:nix-community/disko";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    impermanence.url = "github:nix-community/impermanence";
  };

  outputs = { self, nixpkgs, disko, impermanence }:
  let
    version = nixpkgs.lib.fileContents ./VERSION;
  in {
    nixosModules.chlorum = {
      imports = [
        disko.nixosModules.disko
        impermanence.nixosModules.impermanence
        ./modules/chlorum
      ];
    };

    # The same system for every PC
    nixosConfigurations.chlorum = nixpkgs.lib.nixosSystem {
      modules = [
        self.nixosModules.chlorum
        ./systems/chlorum.nix
        { system.configurationRevision = self.rev or self.dirtyRev or "dirty"; }
      ];
    };

    nixosConfigurations.installer = nixpkgs.lib.nixosSystem {
      modules = [
        "${nixpkgs}/nixos/modules/installer/cd-dvd/installation-cd-minimal.nix"
        ./installer/iso.nix
        self.nixosModules.chlorum
        ({ lib, pkgs, ... }: {
          nixpkgs.hostPlatform = "x86_64-linux";
          chlorum.enable = true;
          image.baseName = lib.mkForce "chlorum-${version}-x86_64";
          boot.supportedFilesystems.zfs = lib.mkForce false;

          # Chlorum itself travels inside the ISO
          environment.etc."chlorum".source = self;

          environment.systemPackages = [
            (pkgs.writeShellApplication {
              name = "chlorum-install";
              runtimeInputs = [
                disko.packages.x86_64-linux.disko
                pkgs.util-linux
                pkgs.gawk
                pkgs.gptfdisk
                pkgs.lvm2
                pkgs.cryptsetup
              ];
              text = builtins.readFile ./installer/chlorum-install.sh;
            })
          ];
        })
      ];
    };

    packages.x86_64-linux.iso =
      self.nixosConfigurations.installer.config.system.build.isoImage;
  };
}
