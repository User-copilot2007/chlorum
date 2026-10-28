{ config, lib, ... }:
{
  options.chlorum.persist.enable = lib.mkEnableOption "Chlorum clean root";

  config = lib.mkIf config.chlorum.persist.enable {
    # /nix and /home must be open for traversal: every service that drops
    # root (D-Bus first of all) and every profile needs to reach them.
    # 0.1.0 created them as 0700; this repairs such installs on every boot.
    systemd.tmpfiles.rules = [
      "z /nix 0755 root root -"
      "z /home 0755 root root -"
    ];

    # The time zone (/etc/localtime, a link the root's tmpfs forgets):
    # kept in /var/lib/chlorum/timezone whenever it changes, put back
    # early at every boot, so the clock is right offline too.
    systemd.services.chlorum-timezone-restore = {
      description = "Put back the time zone";
      wantedBy = [ "sysinit.target" ];
      after = [ "local-fs.target" ];
      before = [ "sysinit.target" "systemd-timedated.service" "automatic-timezoned.service" ];
      unitConfig.DefaultDependencies = false;
      serviceConfig.Type = "oneshot";
      script = ''
        f=/var/lib/chlorum/timezone
        [ -s "$f" ] || exit 0
        tz=$(cat "$f")
        case "$tz" in *..*|/*|"") exit 0 ;; esac
        if [ -e "/etc/zoneinfo/$tz" ]; then
          ln -sfn "/etc/zoneinfo/$tz" /etc/localtime
        fi
      '';
    };
    systemd.paths.chlorum-timezone-save = {
      wantedBy = [ "multi-user.target" ];
      # the link is replaced by a rename over it, which only /etc sees
      pathConfig.PathChanged = "/etc";
    };
    systemd.services.chlorum-timezone-save = {
      description = "Keep the time zone";
      serviceConfig.Type = "oneshot";
      script = ''
        t=$(readlink /etc/localtime) || exit 0
        tz=''${t#*/zoneinfo/}
        if [ "$tz" = "$t" ] || [ -z "$tz" ]; then exit 0; fi
        # only when it changed (anything in /etc wakes this up)
        if [ "$(cat /var/lib/chlorum/timezone 2>/dev/null)" = "$tz" ]; then exit 0; fi
        mkdir -p /var/lib/chlorum
        printf %s "$tz" > /var/lib/chlorum/timezone.new && mv /var/lib/chlorum/timezone.new /var/lib/chlorum/timezone
      '';
    };

    environment.persistence."/persist" = {
      hideMounts = true;
      directories = [
        "/var/log"                                # system logs
        "/var/lib/nixos"                          # stable user and group IDs
        "/var/lib/systemd"                        # timers, random seed, profile records
        "/var/lib/chlorum"                        # first-run state, computer name
        "/etc/NetworkManager/system-connections"  # saved Wi-Fi and VPN
        "/var/lib/NetworkManager"                 # which networks it knows, Wi-Fi on or off
        "/var/lib/bluetooth"                      # paired headphones, keyboards, controllers
        "/var/lib/cups"                           # printers that were added
        "/var/lib/fwupd"                          # firmware update state
        "/var/lib/alsa"                           # sound card levels
        "/var/lib/upower"                         # battery history
      ];
      files = [
        "/etc/machine-id"                         # this PC's identity
      ];
    };
  };
}
