{ config, lib, pkgs, ... }:
let
  cfg = config.chlorum.desktop;

  firefox = config.programs.firefox.package;
  sway = config.programs.sway.package;

  # The Chlorum screens (first run, login, main menu) and their backends
  ui = pkgs.runCommand "chlorum-ui" { nativeBuildInputs = [ pkgs.makeWrapper ]; } ''
    mkdir -p $out/share/chlorum-ui $out/bin
    cp -r ${../../ui}/. $out/share/chlorum-ui/
    chmod -R u+w $out/share/chlorum-ui
    cp ${../../assets/logo/chlorum-icon-small.svg} $out/share/chlorum-ui/logo.svg
    makeWrapper ${pkgs.python3}/bin/python3 $out/bin/chlorum-ui \
      --add-flags $out/share/chlorum-ui/chlorum-ui.py \
      --set CHLORUM_UI_DIR $out/share/chlorum-ui \
      --set CHLORUM_SESSION_CMD /run/current-system/sw/bin/chlorum-session \
      --prefix PATH : ${lib.makeBinPath [
        firefox
        sway
        pkgs.foot
        config.systemd.package
        config.networking.networkmanager.package
        pkgs.xkcdpass
        pkgs.kbd
        pkgs.getent
        pkgs.coreutils
      ]}
  '';

  # Keyboard layout chosen on first start. Installs set up on the console
  # (0.1.x) only have the console keymap, so translate that one.
  layout = ''
    xkb="$(cat /var/lib/chlorum/xkb-layout 2>/dev/null)"
    if [ -z "$xkb" ]; then
      xkb="$(cat /var/lib/chlorum/keymap 2>/dev/null)"
      case "$xkb" in
        uk) xkb=gb ;;
        sg|fr_CH*|de_CH*) xkb=ch ;;
        *) xkb="''${xkb%%-*}" ;;   # fr-latin9 -> fr, de-latin1 -> de
      esac
    fi
    # A layout the keyboard library does not know leaves no working keyboard.
    if [ -z "$xkb" ] || [ ! -e "${pkgs.xkeyboard_config}/share/X11/xkb/symbols/$xkb" ]; then
      xkb=us
    fi
    export XKB_DEFAULT_LAYOUT="$xkb"
  '';

  # Login screen: one Chlorum page in a kiosk window
  greeter = pkgs.writeShellScript "chlorum-greeter" ''
    ${layout}
    export MOZ_ENABLE_WAYLAND=1
    exec ${pkgs.cage}/bin/cage -s -d -- ${ui}/bin/chlorum-ui greeter
  '';

  # What a profile runs after logging in
  session = pkgs.writeShellScriptBin "chlorum-session" ''
    ${layout}
    export XDG_CURRENT_DESKTOP=sway
    export XDG_SESSION_TYPE=wayland
    export MOZ_ENABLE_WAYLAND=1
    exec sway
  '';

  logoPng = pkgs.runCommand "chlorum-logo.png" { nativeBuildInputs = [ pkgs.librsvg ]; } ''
    rsvg-convert -w 160 -h 160 ${../../assets/logo/chlorum-logo.svg} -o $out
  '';
in
{
  options.chlorum.desktop.enable = lib.mkEnableOption ''
    the Chlorum desktop: graphical boot, first run and login, a session with
    Firefox and a terminal side by side, and the main menu on the Super key
  '';

  config = lib.mkIf cfg.enable {
    # ---- Boot: logo instead of text --------------------------------------
    boot.plymouth = {
      enable = true;
      theme = "spinner";
      logo = logoPng;
    };
    boot.kernelParams = [
      "quiet"
      "splash"
      "loglevel=3"
      "rd.systemd.show_status=false"
      "rd.udev.log_level=3"
      "udev.log_level=3"
      "vt.global_cursor_default=0"
    ];
    boot.consoleLogLevel = 0;
    boot.initrd.verbose = false;
    boot.loader.timeout = 0;          # hold Space at power-on for older versions

    # ---- First run and login ---------------------------------------------
    services.greetd = {
      enable = true;
      settings.default_session = {
        user = "greeter";
        command = "${greeter}";
      };
    };

    # Never give up on the login screen: a few quick failures in a row must
    # not leave a black screen.
    systemd.services.greetd = {
      unitConfig.StartLimitIntervalSec = 0;
      serviceConfig.RestartSec = 1;
    };

    # Logging out ends everything the profile started, so its encrypted
    # volume can close.
    services.logind.settings.Login.KillUserProcesses = true;

    systemd.services.chlorum-setupd = {
      description = "Chlorum first-run helper";
      wantedBy = [ "multi-user.target" ];
      before = [ "greetd.service" ];
      wants = [ "systemd-homed.service" "NetworkManager.service" ];
      after = [ "systemd-homed.service" "NetworkManager.service" ];
      unitConfig = {
        ConditionPathExists = "!/var/lib/chlorum/setup-done";
        RequiresMountsFor = "/var/lib/chlorum";
      };
      serviceConfig = {
        ExecStart = "${ui}/bin/chlorum-ui setupd";
        Restart = "on-failure";
      };
    };

    # ---- Session: sway, Firefox left, terminal right, menu on Super ------
    programs.sway = {
      enable = true;
      wrapperFeatures.gtk = true;
      extraPackages = [ ];
    };
    environment.etc."sway/config".text = ''
      set $mod Mod4
      font pango:IBM Plex Mono 10

      output * bg #000000 solid_color
      default_border pixel 2
      default_floating_border pixel 2
      gaps inner 6
      client.focused          #FFD60A #FFD60A #000000 #FFD60A #FFD60A
      client.focused_inactive #262626 #111111 #D4D4D4 #262626 #262626
      client.unfocused        #262626 #111111 #9A9A9A #262626 #262626
      focus_follows_mouse no
      workspace_auto_back_and_forth yes

      # Main menu: its own full-screen workspace, Super shows and hides it
      for_window [app_id="chlorum-menu"] move container to workspace Home, fullscreen enable
      bindsym --release Super_L workspace Home
      bindsym --release Super_R workspace Home

      # Firefox on the left, the terminal on the right
      for_window [app_id="firefox"] move left

      bindsym $mod+Return exec foot
      bindsym $mod+b exec firefox
      bindsym $mod+q kill
      bindsym $mod+f fullscreen toggle
      bindsym $mod+Left focus left
      bindsym $mod+Right focus right
      bindsym $mod+Up focus up
      bindsym $mod+Down focus down
      bindsym $mod+Shift+Left move left
      bindsym $mod+Shift+Right move right
      bindsym $mod+Shift+Up move up
      bindsym $mod+Shift+Down move down
      bindsym $mod+1 workspace number 1
      bindsym $mod+2 workspace number 2
      bindsym $mod+3 workspace number 3
      bindsym $mod+Shift+1 move container to workspace number 1
      bindsym $mod+Shift+2 move container to workspace number 2
      bindsym $mod+Shift+3 move container to workspace number 3

      exec ${pkgs.dbus}/bin/dbus-update-activation-environment --systemd WAYLAND_DISPLAY SWAYSOCK XDG_CURRENT_DESKTOP
      exec chlorum-ui shell
      exec firefox
      exec foot
      exec swaymsg workspace number 1
    '';

    environment.etc."xdg/foot/foot.ini".text = ''
      font=IBM Plex Mono:size=11
      pad=12x12

      [colors]
      background=000000
      foreground=f5f5f5
      regular0=111111
      regular1=ff5c5c
      regular2=8ae234
      regular3=ffd60a
      regular4=6ca0dc
      regular5=c678dd
      regular6=56b6c2
      regular7=d4d4d4
      bright0=3a3a3a
      bright3=ffe566
      bright7=f5f5f5

      [cursor]
      color=000000 ffd60a
    '';

    # ---- Apps: browser, terminal and what it takes to build Chlorum ------
    programs.firefox = {
      enable = true;
      policies = {
        DisableTelemetry = true;
        DisableFirefoxStudies = true;
        DisablePocket = true;
        DontCheckDefaultBrowser = true;
        SkipTermsOfUse = true;
        OverrideFirstRunPage = "";
        OverridePostUpdatePage = "";
        UserMessaging = {
          WhatsNew = false;
          ExtensionRecommendations = false;
          FeatureRecommendations = false;
          SkipOnboarding = true;
          MoreFromMozilla = false;
        };
      };
    };
    environment.systemPackages = [
      ui
      session
      pkgs.foot
      pkgs.micro
      pkgs.git
      pkgs.gh
    ];
    fonts.packages = [ pkgs.ibm-plex ];
  };
}
