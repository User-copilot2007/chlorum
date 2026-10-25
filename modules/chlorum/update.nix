{ config, lib, pkgs, ... }:
let
  cfg = config.chlorum.update;

  chlorum-update = pkgs.writeShellApplication {
    name = "chlorum-update";
    runtimeInputs = [ config.systemd.package pkgs.coreutils ];
    text = ''
      if [ "$(id -u)" -ne 0 ]; then exec run0 "$0" "$@"; fi
      echo "Checking for a new Chlorum version..."
      journalctl -u nixos-upgrade.service -f -n 0 -o cat &
      log=$!
      trap 'kill "$log" 2>/dev/null' EXIT
      if systemctl start nixos-upgrade.service; then
        echo
        echo "Done. If there was a new version, it starts after the next reboot."
      else
        echo
        echo "Update failed. Your current version keeps working."
        echo "Details: journalctl -u nixos-upgrade"
        exit 1
      fi
    '';
  };
in
{
  options.chlorum.update = {
    enable = lib.mkEnableOption "automatic Chlorum updates";
    source = lib.mkOption {
      type = lib.types.str;
      default = "github:User-copilot2007/chlorum#chlorum";
      description = "Where this machine gets new Chlorum versions.";
    };
    dates = lib.mkOption {
      type = lib.types.str;
      default = "daily";
      example = "*-*-* 00/6:00:00";
      description = "How often to check for updates (systemd calendar format).";
    };
  };

  config = lib.mkIf cfg.enable {
    system.autoUpgrade = {
      enable = true;
      flake = cfg.source;
      operation = "boot";          # new version starts on next boot
      dates = cfg.dates;
      randomizedDelaySec = "45min";
      persistent = true;           # catch up if the PC was off
      allowReboot = false;
    };
    environment.systemPackages = [ chlorum-update ];

    # Old versions are removed so updates never fill the disk.
    # The last 14 days stay available in the boot menu.
    nix.gc = {
      automatic = true;
      dates = "weekly";
      options = "--delete-older-than 14d";
    };
    nix.optimise.automatic = true;
    boot.loader.systemd-boot.configurationLimit = lib.mkDefault 10;
  };
}
