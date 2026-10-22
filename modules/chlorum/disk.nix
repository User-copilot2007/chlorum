{ config, lib, ... }:
let
  cfg = config.chlorum.disk;
  btrfsOpts = [ "compress=zstd" "noatime" ];
in
{
  options.chlorum.disk = {
    enable = lib.mkEnableOption "Chlorum disk layout";
    device = lib.mkOption {
      type = lib.types.str;
      default = "/dev/disk/chosen-by-installer";
      description = "Disk to install on. Chosen by the installer, never stored in the repository.";
    };
  };

  config = lib.mkIf cfg.enable {
    disko.devices = {
      nodev."/" = {
        fsType = "tmpfs";
        mountOptions = [ "size=25%" "mode=755" ];
      };

      disk.main = {
        type = "disk";
        device = cfg.device;
        content = {
          type = "gpt";
          partitions = {
            ESP = {
              size = "1G";
              type = "EF00";
              content = {
                type = "filesystem";
                format = "vfat";
                mountpoint = "/boot";
                mountOptions = [ "umask=0077" ];
              };
            };
            system = {
              size = "100%";
              content = {
                type = "luks";
                name = "chlorum";
                passwordFile = "/tmp/chlorum-disk-password";
                settings.allowDiscards = true;
                content = {
                  type = "btrfs";
                  extraArgs = [ "-f" ];
                  subvolumes = {
                    "/nix" = { mountpoint = "/nix"; mountOptions = btrfsOpts; };
                    "/persist" = { mountpoint = "/persist"; mountOptions = btrfsOpts; };
                    "/home" = { mountpoint = "/home"; mountOptions = btrfsOpts; };
                  };
                };
              };
            };
          };
        };
      };
    };

    fileSystems."/persist".neededForBoot = true;
    boot.initrd.luks.devices.chlorum.crypttabExtraOpts = [ "fido2-device=auto" ];
  };
}
