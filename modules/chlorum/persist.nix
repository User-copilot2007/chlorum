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
