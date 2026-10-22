{
  description = "Chlorum";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-26.05";

  outputs = { self, nixpkgs }: {
    nixosModules.chlorum = import ./modules/chlorum;
  };
}
