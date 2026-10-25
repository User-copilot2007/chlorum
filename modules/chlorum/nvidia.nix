{ config, lib, pkgs, ... }:
# NVIDIA graphics (GeForce RTX 20 series and newer) with NVIDIA's open
# kernel driver. The driver is always on board, but its modules load only
# on PCs that actually have an NVIDIA card: nothing is set per machine.
{
  options.chlorum.nvidia.enable = lib.mkEnableOption "NVIDIA graphics support" // {
    default = true;
  };

  config = lib.mkIf (config.chlorum.nvidia.enable && config.chlorum.desktop.enable) {
    nixpkgs.config.allowUnfreePredicate = pkg:
      builtins.elem (lib.getName pkg) [ "nvidia-x11" "nvidia-settings" "nvidia-persistenced" ];

    hardware.graphics.enable = true;
    services.xserver.videoDrivers = [ "nvidia" ];
    hardware.nvidia = {
      open = true;
      modesetting.enable = true;
      nvidiaSettings = false;
      package = config.boot.kernelPackages.nvidiaPackages.stable;
    };

    # Early graphics, so the boot logo and the login screen stay smooth.
    # In the initrd but loaded only when the card is present.
    boot.initrd.availableKernelModules = [ "nvidia" "nvidia_modeset" "nvidia_drm" "nvidia_uvm" ];
    boot.extraModprobeConfig = ''
      softdep nvidia post: nvidia-modeset nvidia-drm
    '';
  };
}
