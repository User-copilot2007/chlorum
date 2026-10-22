{
  description = "Chlorum";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-26.05";

  outputs = { self, nixpkgs }: {
    nixosModules.chlorum = import ./modules/chlorum;

    nixosConfigurations.installer = nixpkgs.lib.nixosSystem {
      modules = [
        "${nixpkgs}/nixos/modules/installer/cd-dvd/installation-cd-minimal.nix"
        self.nixosModules.chlorum
        ({ lib, ... }: {
          nixpkgs.hostPlatform = "x86_64-linux";
          chlorum.enable = true;
          image.baseName = lib.mkForce "chlorum-0.0.2-x86_64";
          boot.supportedFilesystems.zfs = lib.mkForce false;
        })
      ];
    };

    packages.x86_64-linux.iso =
      self.nixosConfigurations.installer.config.system.build.isoImage;
  };
}
