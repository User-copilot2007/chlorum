{
  description = "Chlorum";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-26.05";

  outputs = { self, nixpkgs }: {
    nixosModules.chlorum = import ./modules/chlorum;

    nixosConfigurations.installer = nixpkgs.lib.nixosSystem {
      modules = [
        "${nixpkgs}/nixos/modules/installer/cd-dvd/installation-cd-minimal.nix"
        self.nixosModules.chlorum
        {
          nixpkgs.hostPlatform = "x86_64-linux";
          chlorum.enable = true;
        }
      ];
    };

    packages.x86_64-linux.iso =
      self.nixosConfigurations.installer.config.system.build.isoImage;
  };
}
