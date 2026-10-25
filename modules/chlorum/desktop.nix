{ config, lib, pkgs, ... }:
let
  cfg = config.chlorum.desktop;
  version = lib.fileContents ../../VERSION;

  # Chlorum Home: the main menu on the Super key (native, libcosmic)
  home = pkgs.rustPlatform.buildRustPackage {
    pname = "chlorum-home";
    inherit version;
    src = ../../apps/chlorum-home;
    cargoLock = {
      lockFile = ../../apps/chlorum-home/Cargo.lock;
      allowBuiltinFetchGit = true;
    };
    nativeBuildInputs = [ pkgs.libcosmicAppHook pkgs.pkg-config ];
    buildInputs = [ pkgs.fontconfig pkgs.freetype ];
    postInstall = ''
      install -Dm644 ${../../assets/logo/chlorum-icon-small.svg} \
        $out/share/icons/hicolor/scalable/apps/com.chlorum.Logo.svg
    '';
    meta.mainProgram = "chlorum-home";
  };

  # The Chlorum look as COSMIC system defaults: black, yellow, square,
  # IBM Plex, tiling, clock with seconds. Everything stays changeable in
  # COSMIC Settings; a person's own choices override these.
  defaults = pkgs.runCommand "chlorum-cosmic-defaults" { } ''
    mkdir -p $out/share/cosmic
    cp -r ${../../assets/cosmic}/. $out/share/cosmic/
  '';

  # Settings that COSMIC packages already ship themselves are set per
  # person on first login instead, and only if the person has none yet.
  firstLogin = pkgs.writeShellScript "chlorum-start" ''
    c="''${XDG_CONFIG_HOME:-$HOME/.config}/cosmic"
    seed() {
      if [ ! -e "$c/$1" ]; then
        mkdir -p "$(dirname "$c/$1")"
        printf '%s' "$2" > "$c/$1"
      fi
    }
    # Super opens Chlorum Home instead of the launcher
    seed com.system76.CosmicSettings.Shortcuts/v1/system_actions \
      '{ Launcher: "chlorum-home", }'
    # Plain black desktop
    seed com.system76.CosmicBackground/v1/all \
      '(output: "all", source: Color(Single((0.0, 0.0, 0.0))), filter_by_theme: false, rotation_frequency: 3600, filter_method: Lanczos, scaling_mode: Zoom, sampling_method: Alphanumeric)'

    # Firefox on the left, the terminal on the right (COSMIC tiles them)
    firefox &
    sleep 2
    cosmic-term &
  '';

  autostart = pkgs.writeTextDir "etc/xdg/autostart/chlorum-start.desktop" ''
    [Desktop Entry]
    Type=Application
    Name=Chlorum start
    Exec=${firstLogin}
    NoDisplay=true
  '';

  # Login screen. Until the first run is done, the Chlorum first run is
  # shown instead (keyboard, network, profile), then COSMIC's own login.
  firstRunUi = pkgs.runCommand "chlorum-ui" { nativeBuildInputs = [ pkgs.makeWrapper ]; } ''
    mkdir -p $out/share/chlorum-ui $out/bin
    cp -r ${../../ui}/. $out/share/chlorum-ui/
    chmod -R u+w $out/share/chlorum-ui
    cp ${../../assets/logo/chlorum-icon-small.svg} $out/share/chlorum-ui/logo.svg
    makeWrapper ${pkgs.python3}/bin/python3 $out/bin/chlorum-ui \
      --add-flags $out/share/chlorum-ui/chlorum-ui.py \
      --set CHLORUM_UI_DIR $out/share/chlorum-ui \
      --set CHLORUM_GREETER_GROUP cosmic-greeter \
      --prefix PATH : ${lib.makeBinPath [
        config.programs.firefox.package
        config.systemd.package
        config.networking.networkmanager.package
        pkgs.xkcdpass
        pkgs.kbd
        pkgs.getent
        pkgs.coreutils
      ]}
  '';
  cosmicGreeterPkg = config.services.displayManager.cosmic-greeter.package;
  greeter = pkgs.writeShellScript "chlorum-greeter" ''
    if [ -e /var/lib/chlorum/setup-done ]; then
      export XCURSOR_THEME="''${XCURSOR_THEME:-Pop}"
      exec ${lib.getExe' cosmicGreeterPkg "cosmic-greeter-start"}
    fi
    export XKB_DEFAULT_LAYOUT="$(cat /var/lib/chlorum/xkb-layout 2>/dev/null || echo us)"
    [ -e "${pkgs.xkeyboard_config}/share/X11/xkb/symbols/$XKB_DEFAULT_LAYOUT" ] || XKB_DEFAULT_LAYOUT=us
    export MOZ_ENABLE_WAYLAND=1
    exec ${pkgs.cage}/bin/cage -s -d -- ${firstRunUi}/bin/chlorum-ui greeter
  '';

  logoPng = pkgs.runCommand "chlorum-logo.png" { nativeBuildInputs = [ pkgs.librsvg ]; } ''
    rsvg-convert -w 160 -h 160 ${../../assets/logo/chlorum-logo.svg} -o $out
  '';
in
{
  options.chlorum.desktop.enable = lib.mkEnableOption ''
    the Chlorum desktop: COSMIC with the Chlorum look, Chlorum Home on the
    Super key, graphical boot, first run and login
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

    # ---- Desktop: COSMIC ---------------------------------------------------
    services.desktopManager.cosmic.enable = true;
    services.displayManager.cosmic-greeter.enable = true;
    # Chlorum has its own first run; COSMIC's welcome wizard would repeat it.
    environment.cosmic.excludePackages = [ pkgs.cosmic-initial-setup ];
    services.desktopManager.cosmic.showExcludedPkgsWarning = false;

    services.greetd.settings.default_session.command = lib.mkForce "${greeter}";
    # Encrypted profiles (systemd-homed) get user IDs 60001-60513. The login
    # screen lists only IDs up to UID_MAX (29999 by default), so raise it.
    security.loginDefs.settings.UID_MAX = 60513;
    systemd.services.greetd = {
      unitConfig.StartLimitIntervalSec = 0;
      serviceConfig.RestartSec = 1;
    };

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
      environment.CHLORUM_GREETER_GROUP = "cosmic-greeter";
      serviceConfig = {
        ExecStart = "${firstRunUi}/bin/chlorum-ui setupd";
        Restart = "on-failure";
      };
    };

    # Logging out ends everything the profile started, so its encrypted
    # volume can close.
    services.logind.settings.Login.KillUserProcesses = true;

    # ---- Apps --------------------------------------------------------------
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
      home
      defaults
      autostart
      pkgs.git
      pkgs.gh
    ];
    fonts.packages = [ pkgs.ibm-plex ];
  };
}
