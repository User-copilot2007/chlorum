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
        ({ lib, ... }: {
          nixpkgs.hostPlatform = "x86_64-linux";
          chlorum.enable = true;
          image.baseName = lib.mkForce "chlorum-0.0.4-x86_64";
          boot.supportedFilesystems.zfs = lib.mkForce false;
        })
      ];
    };

    packages.x86_64-linux.iso =
      self.nixosConfigurations.installer.config.system.build.isoImage;
  };
}
