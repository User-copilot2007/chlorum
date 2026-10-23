{ modulesPath, ... }:
{
  imports = [ "${modulesPath}/profiles/all-hardware.nix" ];

  nixpkgs.hostPlatform = "x86_64-linux";

  chlorum.enable = true;
  chlorum.disk.enable = true;
  chlorum.persist.enable = true;
  chlorum.update.enable = true;

  boot.loader.systemd-boot.enable = true;
  boot.loader.efi.canTouchEfiVariables = false;
  boot.initrd.systemd.enable = true;

  hardware.enableRedistributableFirmware = true;
  networking.networkmanager.enable = true;

  # Temporary admin login until profiles. The installer writes only the hash.
  users.mutableUsers = false;
  users.users.root.hashedPasswordFile = "/persist/etc/chlorum/root.hash";

  system.stateVersion = "26.05";
}
