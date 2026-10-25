{ lib, pkgs, ... }:
let
  logo = ../assets/logo/chlorum-logo.svg;

  grubTheme = pkgs.runCommand "chlorum-grub-theme" {
    nativeBuildInputs = [ pkgs.librsvg pkgs.imagemagick ];
  } ''
    mkdir -p $out
    rsvg-convert -w 256 -h 256 ${logo} -o $out/logo.png
    # GRUB draws the selection bar from nine pieces; all must exist
    for p in nw n ne w c e sw s se; do
      # PNG32: full-colour RGBA. A one-colour image is otherwise saved as a
      # 1-bit palette PNG, which GRUB cannot read, and the bar vanishes.
      magick -size 8x8 xc:'#FFD60A' PNG32:$out/select_$p.png
    done
    cat > $out/theme.txt <<'THEME'
    title-text: ""
    desktop-color: "#000000"

    + image {
      left = 50%-128
      top = 6%
      file = "logo.png"
    }

    + boot_menu {
      left = 30%
      top = 40%
      width = 40%
      height = 45%
      item_color = "#D4D4D4"
      selected_item_color = "#000000"
      selected_item_pixmap_style = "select_*.png"
      item_height = 40
      item_padding = 12
      item_spacing = 6
      icon_width = 0
      icon_height = 0
    }

    + label {
      id = "__timeout__"
      top = 92%
      left = 0
      width = 100%
      align = "center"
      text = "Starting in %d s"
      color = "#9A9A9A"
    }
    THEME
  '';
in
{
  # Live user is "chlorum", not "nixos"
  users.users.nixos.enable = false;
  users.users.chlorum = {
    isNormalUser = true;
    extraGroups = [ "wheel" "networkmanager" "video" ];
    initialHashedPassword = "";
  };
  services.getty.autologinUser = lib.mkForce "chlorum";

  # Boot menu and drive name in Chlorum style
  isoImage.grubTheme = grubTheme;
  isoImage.volumeID = "CHLORUM";
}
