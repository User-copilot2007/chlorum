{ config, lib, pkgs, ... }:
let
  version = lib.fileContents ../../VERSION;
in
{
  imports = [ ./disk.nix ./persist.nix ./look.nix ./update.nix ./profiles.nix ./keys.nix ./desktop.nix ./nvidia.nix ];

  options.chlorum.enable = lib.mkEnableOption "Chlorum";

  config = lib.mkIf config.chlorum.enable {
    environment.etc."chlorum-release".text = "Chlorum ${version}\n";
    # Shown in boot menus, the console banner and nixos-version
    system.nixos.label = version;
    system.nixos.distroName = "Chlorum";
    system.nixos.distroId = "chlorum";
    environment.systemPackages = [ pkgs.git ];
    nix.settings.experimental-features = [ "nix-command" "flakes" ];
  };
}
