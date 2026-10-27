{ config, lib, pkgs, ... }:
let
  cfg = config.chlorum.update;

  # chlorum-update           check, then install if there is something new
  # chlorum-update check     "available 0.10.0" or "up-to-date 0.9.0"
  # chlorum-update apply     starts the install in the background (the
  #                          owner needs no password); progress in
  #                          /run/chlorum-update/update.json
  # chlorum-update install   the install itself (root; the service runs it)
  progress = "/run/chlorum-update/update.json";
  chlorum-update = pkgs.writeShellApplication {
    name = "chlorum-update";
    runtimeInputs = [ config.systemd.package config.nix.package pkgs.jq pkgs.coreutils config.chlorum.package ];
    text = ''
      source_flake="${lib.head (lib.splitString "#" cfg.source)}"
      attr="${lib.last (lib.splitString "#" cfg.source)}"
      P="${progress}"

      # rev, then version, of what the update server has now
      remote() {
        local meta path
        meta=$(nix flake metadata --refresh --json "$source_flake") || return 1
        rev=$(jq -r .locked.rev <<<"$meta")
        path=$(jq -r .path <<<"$meta")
        version=$(cat "$path/VERSION" 2>/dev/null || echo "")
        [ -n "$rev" ] && [ "$rev" != null ]
      }

      check() {
        local installed
        if ! remote; then
          echo "Could not reach the Chlorum update server." >&2
          return 1
        fi
        installed=$(cat /run/current-system/configuration-revision 2>/dev/null || echo unknown)
        if [ "$rev" = "$installed" ]; then echo "up-to-date $version"; else echo "available $version"; fi
      }

      install() {
        if ! remote; then
          chlorum update-progress "$P" "failed" "" "Could not reach the Chlorum update server."
          return 1
        fi
        # already running, or already waiting for the next start: nothing to do
        local running staged
        running=$(cat /run/current-system/configuration-revision 2>/dev/null || true)
        staged=$(cat /nix/var/nix/profiles/system/configuration-revision 2>/dev/null || true)
        if [ "$rev" = "$running" ] || [ "$rev" = "$staged" ]; then
          return 0
        fi
        chlorum update-progress "$P" "checking" "$version"
        # whatever stops the install from here on shows as failed, never as
        # "Installing..." forever
        set -o errtrace
        trap 'chlorum update-progress "$P" "failed" "$version" "The install stopped. Nothing changed; try again later."' ERR
        # Nix tells what it downloads and builds; chlorum turns it into a line
        local fifo out ok=0
        fifo=$(mktemp -u)
        mkfifo "$fifo"
        chlorum update-progress "$P" "downloading" "$version" < "$fifo" &
        local writer=$!
        if out=$(nix build --no-link --print-out-paths --log-format internal-json \
              "$source_flake#nixosConfigurations.$attr.config.system.build.toplevel" 2>"$fifo"); then
          ok=1
        fi
        wait "$writer" || true
        rm -f "$fifo"
        if [ "$ok" != 1 ] || [ -z "$out" ]; then
          trap - ERR
          chlorum update-progress "$P" "failed" "$version" "$(jq -r '.message // ""' "$P" 2>/dev/null)"
          return 1
        fi
        chlorum update-progress "$P" "installing" "$version"
        nix-env -p /nix/var/nix/profiles/system --set "$out"
        "$out/bin/switch-to-configuration" boot
        trap - ERR
        chlorum update-progress "$P" "done" "$version"
      }

      case "''${1:-}" in
        check) check ;;
        apply) systemctl start --no-block chlorum-update.service ;;
        install) install ;;
        "")
          echo "Checking for a new Chlorum version..."
          state=$(check)
          if [ "''${state%% *}" = up-to-date ]; then
            echo "Chlorum is up to date (''${state#* })."
            exit 0
          fi
          echo "Chlorum ''${state#* } is available. Installing; this can take a while..."
          if systemctl start chlorum-update.service; then
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
    # Installs the newest version for the next start. Run by the timer, and
    # by the owner from the system menu or Center (the line shows how far
    # it is).
    systemd.services.chlorum-update = {
      description = "Chlorum update: install the newest version for the next start";
      restartIfChanged = false;
      unitConfig.X-StopOnRemoval = false;
      after = [ "network-online.target" ];
      wants = [ "network-online.target" ];
      environment.HOME = "/root";
      serviceConfig = {
        Type = "oneshot";
        ExecStart = "${chlorum-update}/bin/chlorum-update install";
        # the progress file is read by the shell, and kept until restart
        RuntimeDirectory = "chlorum-update";
        RuntimeDirectoryMode = "0755";
        RuntimeDirectoryPreserve = "yes";
        Nice = 10;
        IOSchedulingClass = "idle";
      };
    };
    systemd.timers.chlorum-update = {
      wantedBy = [ "timers.target" ];
      timerConfig = {
        OnCalendar = cfg.dates;
        RandomizedDelaySec = "45min";
        Persistent = true;           # catch up if the PC was off
      };
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
            action.lookup("unit") == "chlorum-update.service" &&
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
