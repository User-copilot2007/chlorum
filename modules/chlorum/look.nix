{ config, lib, pkgs, ... }:
{
  config = lib.mkIf config.chlorum.enable {
    # Console greeting: black and yellow, no NixOS help line
    environment.etc.issue.source = lib.mkForce (pkgs.writeText "issue" ''

      \e{yellow}\e{bold}CHLORUM ${config.system.nixos.label}\e{reset}  \l

    '');

    # Yellow prompt for every user
    programs.bash.promptInit = ''
      PS1="\n\[\033[1;33m\][\u@\h:\w]\$\[\033[0m\] "
    '';
  };
}
