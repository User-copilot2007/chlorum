{ modulesPath, ... }:
{
  imports = [ "${modulesPath}/profiles/all-hardware.nix" ];

  nixpkgs.hostPlatform = "x86_64-linux";

  chlorum.enable = true;
  chlorum.disk.enable = true;
  chlorum.persist.enable = true;

  boot.loader.systemd-boot.enable = true;
  boot.loader.efi.canTouchEfiVariables = false;
  boot.initrd.systemd.enable = true;

  hardware.enableRedistributableFirmware = true;
  networking.networkmanager.enable = true;

  system.stateVersion = "26.05";
}
