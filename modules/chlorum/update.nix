{ config, lib, ... }:
let
  cfg = config.chlorum.update;
in
{
  options.chlorum.update = {
    enable = lib.mkEnableOption "automatic Chlorum updates";
    source = lib.mkOption {
      type = lib.types.str;
      default = "github:User-copilot2007/chlorum#chlorum";
      description = "Where this machine gets new Chlorum versions.";
    };
  };

  config = lib.mkIf cfg.enable {
    system.autoUpgrade = {
      enable = true;
      flake = cfg.source;
      operation = "boot";          # new version starts on next boot
      dates = "daily";
      randomizedDelaySec = "45min";
      persistent = true;           # catch up if the PC was off
      allowReboot = false;
    };
  };
}
