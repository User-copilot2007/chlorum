{ config, lib, pkgs, ... }:
{
  imports = [ ./disk.nix ./persist.nix ./look.nix ];

  options.chlorum.enable = lib.mkEnableOption "Chlorum";

  config = lib.mkIf config.chlorum.enable {
    environment.etc."chlorum-release".text = "Chlorum 0.0.9\n";
    system.nixos.distroName = "Chlorum";
    system.nixos.distroId = "chlorum";
    environment.systemPackages = [ pkgs.git ];
    nix.settings.experimental-features = [ "nix-command" "flakes" ];
  };
}
