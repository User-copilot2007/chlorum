{ config, lib, pkgs, ... }:
let
  cfg = config.chlorum.update;

  # chlorum-update           check, then install if there is something new
  # chlorum-update check     prints "up-to-date" or "available"
  # chlorum-update apply     installs now (no password for the owner)
  chlorum-update = pkgs.writeShellApplication {
    name = "chlorum-update";
    runtimeInputs = [ config.systemd.package config.nix.package pkgs.jq pkgs.coreutils ];
    text = ''
      source_flake="${lib.head (lib.splitString "#" cfg.source)}"

      check() {
        local remote installed
        remote=$(nix flake metadata --refresh --json "$source_flake" | jq -r .locked.rev)
        installed=$(cat /run/current-system/configuration-revision 2>/dev/null || echo unknown)
        if [ -z "$remote" ] || [ "$remote" = null ]; then
          echo "Could not reach the Chlorum update server." >&2
          return 1
        fi
        if [ "$remote" = "$installed" ]; then echo up-to-date; else echo available; fi
      }

      apply() {
        systemctl start nixos-upgrade.service
      }

      case "''${1:-}" in
        check) check ;;
        apply) apply ;;
        "")
          echo "Checking for a new Chlorum version..."
          state=$(check)
          if [ "$state" = up-to-date ]; then
            echo "Chlorum is up to date."
            exit 0
          fi
          echo "A new version is available. Installing; this can take a while..."
          if apply; then
            echo "Done. The new version starts after the next restart."
          else
            echo "The update failed. Your current version keeps working."
            exit 1
          fi
          ;;
        *) echo "usage: chlorum-update [check|apply]" >&2; exit 2 ;;
      esac
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

    # Which Chlorum commit is installed, so updates can tell if anything is new
    system.systemBuilderCommands = ''
      echo -n ${lib.escapeShellArg (toString config.system.configurationRevision)} > $out/configuration-revision
    '';

    # The owner starts updates without a password, but nothing else.
    security.polkit.extraConfig = ''
      polkit.addRule(function(action, subject) {
        if (action.id == "org.freedesktop.systemd1.manage-units" &&
            action.lookup("unit") == "nixos-upgrade.service" &&
            (action.lookup("verb") == "start" || action.lookup("verb") == "restart") &&
            subject.isInGroup("chlorum-owner")) {
          return polkit.Result.YES;
        }
      });
      // The person at the computer may join Wi-Fi networks from Center
      // (Chlorum has no password pop-ups for this).
      polkit.addRule(function(action, subject) {
        if (action.id.indexOf("org.freedesktop.NetworkManager.") == 0 &&
            subject.isInGroup("chlorum-owner") && subject.active && subject.local) {
          return polkit.Result.YES;
        }
      });
    '';

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
