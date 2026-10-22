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

  outputs = { self, nixpkgs, disko, impermanence }: {
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
      ];
    };

    nixosConfigurations.installer = nixpkgs.lib.nixosSystem {
      modules = [
        "${nixpkgs}/nixos/modules/installer/cd-dvd/installation-cd-minimal.nix"
        self.nixosModules.chlorum
        ({ lib, pkgs, ... }: {
          nixpkgs.hostPlatform = "x86_64-linux";
          chlorum.enable = true;
          image.baseName = lib.mkForce "chlorum-0.0.7-x86_64";
          boot.supportedFilesystems.zfs = lib.mkForce false;

          # Chlorum itself travels inside the ISO
          environment.etc."chlorum".source = self;

          environment.systemPackages = [
            (pkgs.writeShellApplication {
              name = "chlorum-install";
              runtimeInputs = [
                disko.packages.x86_64-linux.disko
                pkgs.mkpasswd
                pkgs.util-linux
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
