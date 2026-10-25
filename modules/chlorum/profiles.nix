{ config, lib, pkgs, ... }:
let
  cfg = config.chlorum.profiles;

  chlorum-setup = pkgs.writeShellApplication {
    name = "chlorum-setup";
    runtimeInputs = [
      config.systemd.package
      config.networking.networkmanager.package
      pkgs.cryptsetup
      pkgs.xkcdpass
      pkgs.kbd
      pkgs.coreutils
      pkgs.gnugrep
      pkgs.bashInteractive
    ];
    text = builtins.readFile ./scripts/chlorum-setup.sh;
  };
in
{
  options.chlorum.profiles.enable = lib.mkEnableOption ''
    encrypted profiles: every person gets their own encrypted volume
    (systemd-homed), created on first start without a root account
  '';

  config = lib.mkIf cfg.enable {
    # Profiles: one LUKS2 volume per person, opened by their own password
    services.homed = {
      enable = true;
      promptOnFirstBoot = false;       # Chlorum's own first run does this
      settings.Home = {
        DefaultStorage = "luks";
        DefaultFileSystemType = "btrfs";
      };
    };

    # Nix's build users have high UIDs; homed's own first-boot prompt would
    # count them as people. Chlorum runs its own first run, so this is safe.
    services.userdbd.silenceHighSystemUsers = true;

    # Least privilege: nobody is root. The owner approves system changes
    # one at a time with their password (run0 and polkit), no sudo, no wheel.
    users.mutableUsers = false;
    users.users.root.hashedPassword = "!";
    # People log in with homed profiles, which NixOS does not see as
    # declared passwords. Without this NixOS refuses a system with no root login.
    users.allowNoPasswordLogin = true;
    users.groups.chlorum-owner = { };
    security.sudo.enable = false;
    security.polkit.enable = true;
    security.polkit.adminIdentities = [ "unix-group:chlorum-owner" ];
    # run0 waits for the password over D-Bus, which gives up after 25 s by
    # default: too short to type a five-word password. Allow 2 minutes.
    environment.variables.SYSTEMD_BUS_TIMEOUT = "120";

    # Time zone follows the network location
    services.automatic-timezoned.enable = true;

    # Computer name: generated on first start, kept in /var/lib/chlorum
    networking.hostName = "";
    system.name = "chlorum";
    networking.networkmanager.settings.main.hostname-mode = "none";
    systemd.services.chlorum-hostname = {
      description = "Set the Chlorum computer name";
      wantedBy = [ "multi-user.target" ];
      before = [ "network-pre.target" "NetworkManager.service" ];
      wants = [ "network-pre.target" ];
      unitConfig.RequiresMountsFor = "/var/lib/chlorum";
      serviceConfig.Type = "oneshot";
      script = ''
        if [ -s /var/lib/chlorum/hostname ]; then
          cat /var/lib/chlorum/hostname > /proc/sys/kernel/hostname
        fi
      '';
    };

    # Keyboard layout chosen on first start, applied before every login
    systemd.services.chlorum-keymap = {
      description = "Apply the Chlorum keyboard layout";
      wantedBy = [ "multi-user.target" ];
      after = [ "systemd-vconsole-setup.service" ];
      before = [ "getty@tty1.service" "chlorum-setup.service" ];
      unitConfig.RequiresMountsFor = "/var/lib/chlorum";
      path = [ pkgs.kbd ];
      serviceConfig.Type = "oneshot";
      script = ''
        if [ -s /var/lib/chlorum/keymap ]; then
          loadkeys -q "$(cat /var/lib/chlorum/keymap)"
        fi
      '';
    };

    # First run: on tty1, before the login prompt, until it has finished
    systemd.services.chlorum-setup = {
      description = "Chlorum first-run setup";
      wantedBy = [ "multi-user.target" ];
      wants = [ "systemd-homed.service" "NetworkManager.service" ];
      after = [
        "systemd-homed.service"
        "NetworkManager.service"
        "systemd-user-sessions.service"
        "chlorum-hostname.service"
      ];
      before = [ "getty@tty1.service" ];
      unitConfig = {
        ConditionPathExists = "!/var/lib/chlorum/setup-done";
        RequiresMountsFor = "/var/lib/chlorum";
      };
      environment.TERM = "linux";      # for the Wi-Fi picker (nmtui)
      serviceConfig = {
        Type = "oneshot";
        RemainAfterExit = true;
        ExecStart = "${chlorum-setup}/bin/chlorum-setup";
        StandardInput = "tty-force";
        StandardOutput = "tty";
        StandardError = "tty";
        TTYPath = "/dev/tty1";
        TTYReset = true;
        TTYVHangup = true;
        TTYVTDisallocate = true;
        TimeoutStartSec = "infinity";
      };
    };
  };
}
