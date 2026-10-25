{ config, lib, pkgs, ... }:
let
  chlorum-keys = pkgs.writeShellApplication {
    name = "chlorum-keys";
    runtimeInputs = [
      config.systemd.package
      pkgs.cryptsetup
      pkgs.coreutils
      pkgs.gnugrep
    ];
    text = builtins.readFile ./scripts/chlorum-keys.sh;
  };
in
{
  config = lib.mkIf config.chlorum.profiles.enable {
    environment.systemPackages = [ chlorum-keys ];
  };
}
