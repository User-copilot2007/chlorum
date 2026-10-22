{ config, lib, pkgs, ... }:
{
  options.chlorum.enable = lib.mkEnableOption "Chlorum";

  config = lib.mkIf config.chlorum.enable {
    system.nixos.distroName = "Chlorum";
    system.nixos.distroId = "chlorum";
    environment.etc."chlorum-release".text = "Chlorum 0.0.1\n";
    environment.systemPackages = [ pkgs.git ];
    nix.settings.experimental-features = [ "nix-command" "flakes" ];
  };
}
