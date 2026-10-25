{ config, lib, ... }:
{
  options.chlorum.persist.enable = lib.mkEnableOption "Chlorum clean root";

  config = lib.mkIf config.chlorum.persist.enable {
    # dbus-broker is known to fail to start on systems whose root is wiped
    # on every boot (nixpkgs issue 302771). The classic D-Bus daemon works.
    services.dbus.implementation = "dbus";

    environment.persistence."/persist" = {
      hideMounts = true;
      directories = [
        "/var/log"                                # system logs
        "/var/lib/nixos"                          # stable user and group IDs
        "/var/lib/systemd"                        # timers, random seed, profile records
        "/var/lib/chlorum"                        # first-run state, computer name
        "/etc/NetworkManager/system-connections"  # saved Wi-Fi and VPN
      ];
      files = [
        "/etc/machine-id"                         # this PC's identity
      ];
    };
  };
}
