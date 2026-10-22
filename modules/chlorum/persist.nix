{ config, lib, ... }:
{
  options.chlorum.persist.enable = lib.mkEnableOption "Chlorum clean root";

  config = lib.mkIf config.chlorum.persist.enable {
    environment.persistence."/persist" = {
      hideMounts = true;
      directories = [
        "/var/log"                                # system logs
        "/var/lib/nixos"                          # stable user and group IDs
        "/var/lib/systemd"                        # timers, random seed, user records
        "/etc/NetworkManager/system-connections"  # saved Wi-Fi and VPN
      ];
      files = [
        "/etc/machine-id"                         # this PC's identity
      ];
    };
  };
}
