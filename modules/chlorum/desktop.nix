{ config, lib, pkgs, ... }:
let
  cfg = config.chlorum.desktop;
  version = lib.fileContents ../../VERSION;

  # ---------------------------------------------------------------- programs

  # The shell (Home, system menu, the two layouts), the login screen and
  # Library: one Rust program on libcosmic.
  chlorum = pkgs.rustPlatform.buildRustPackage {
    pname = "chlorum";
    inherit version;
    src = ../../apps/chlorum;
    cargoLock = {
      lockFile = ../../apps/chlorum/Cargo.lock;
      allowBuiltinFetchGit = true;
    };
    nativeBuildInputs = [ pkgs.libcosmicAppHook pkgs.pkg-config ];
    buildInputs = [ pkgs.fontconfig pkgs.freetype ];
    # the template logic has unit tests; they need nothing graphical
    doCheck = true;
    checkFlags = [ "layout::" ];
    postInstall = ''
      install -Dm644 ${../../assets/logo/chlorum-logo.svg} \
        $out/share/icons/hicolor/scalable/apps/com.chlorum.Logo.svg
      install -Dm644 /dev/stdin $out/share/applications/chlorum-library.desktop <<EOF
      [Desktop Entry]
      Type=Application
      Name=Library
      Comment=Your files, sorted for you
      Icon=system-file-manager
      Exec=$out/bin/chlorum library
      StartupWMClass=chlorum-library
      Categories=Utility;FileManager;
      EOF
    '';
    meta.mainProgram = "chlorum";
  };

  # Live wallpapers drawn by the graphics card (GLSL shaders on the
  # background layer). Settings: ~/.config/chlorum/wallpaper, set in Center.
  wallpaper = pkgs.rustPlatform.buildRustPackage {
    pname = "chlorum-wallpaper";
    inherit version;
    src = ../../apps/chlorum-wallpaper;
    cargoLock.lockFile = ../../apps/chlorum-wallpaper/Cargo.lock;
    nativeBuildInputs = [ pkgs.pkg-config ];
    buildInputs = [ pkgs.libGL pkgs.wayland ];
    meta.mainProgram = "chlorum-wallpaper";
  };

  # ---------------------------------------------------------------- browser

  # Chlorum Keys: arrows move between buttons and links on every web page,
  # F puts a letter on each one.
  keysXpi = pkgs.runCommand "chlorum-keys-${version}" { nativeBuildInputs = [ pkgs.zip ]; } ''
    mkdir -p $out
    cd ${../../apps/chlorum-keys}
    zip -X -r $out/keys@chlorum.xpi manifest.json keys.js keys.css
  '';

  # LibreWolf is Firefox with privacy settings, built from the same code on
  # the same day. Unlike Firefox it runs Chlorum's own add-on without a
  # Mozilla signature. A few of its strictest defaults are relaxed so that
  # sites keep you signed in and look normal; everything stays changeable.
  browser = pkgs.wrapFirefox pkgs.librewolf-unwrapped {
    inherit (pkgs.librewolf-unwrapped) extraPrefsFiles extraPoliciesFiles;
    libName = "librewolf";
    extraPolicies = {
      ExtensionSettings."keys@chlorum" = {
        installation_mode = "force_installed";
        install_url = "file://${keysXpi}/keys@chlorum.xpi";
      };
      DontCheckDefaultBrowser = true;
      OverrideFirstRunPage = "";
      OverridePostUpdatePage = "";
    };
    extraPrefs = ''
      // Chlorum Keys is Chlorum's own add-on, installed from the system
      lockPref("xpinstall.signatures.required", false);
      // Chlorum: stay signed in, real time zone and dark pages, normal sites
      defaultPref("privacy.resistFingerprinting", false);
      defaultPref("privacy.resistFingerprinting.letterboxing", false);
      defaultPref("privacy.clearOnShutdown.cookies", false);
      defaultPref("privacy.clearOnShutdown.history", false);
      defaultPref("privacy.clearOnShutdown.sessions", false);
      defaultPref("privacy.clearOnShutdown.offlineApps", false);
      defaultPref("privacy.clearOnShutdown_v2.cookiesAndStorage", false);
      defaultPref("privacy.clearOnShutdown_v2.historyFormDataAndDownloads", false);
      defaultPref("privacy.clearOnShutdown_v2.browsingHistoryAndDownloads", false);
      defaultPref("privacy.sanitize.sanitizeOnShutdown", false);
      defaultPref("browser.sessionstore.resume_from_crash", true);
      defaultPref("webgl.disabled", false);
      defaultPref("layout.css.prefers-color-scheme.content-override", 0);
      defaultPref("browser.download.dir", "");
      defaultPref("browser.download.useDownloadDir", true);
      defaultPref("browser.download.always_ask_before_handling_new_types", false);
      defaultPref("toolkit.legacyUserProfileCustomizations.stylesheets", true);
    '';
  };

  # ---------------------------------------------------------------- session

  # The COSMIC look for the COSMIC apps Chlorum still uses (terminal, text
  # editor): black, yellow, square, IBM Plex.
  cosmicDefaults = pkgs.runCommand "chlorum-cosmic-defaults" { } ''
    mkdir -p $out/share/cosmic
    cp -r ${../../assets/cosmic}/. $out/share/cosmic/
  '';

  # Started by the login screen: niri with the Chlorum rules.
  session = pkgs.writeShellScriptBin "chlorum-session" ''
    dir="''${XDG_RUNTIME_DIR:-/tmp}/chlorum"
    mkdir -p "$dir"

    # keyboard: the layout chosen on first start, plus US for shortcuts
    layout=$(cat /var/lib/chlorum/xkb-layout 2>/dev/null || echo us)
    case "$layout" in
      us|"") layout=us ;;
      *) layout="$layout,us" ;;
    esac
    sed "s/@LAYOUT@/$layout/" ${../../assets/niri/config.kdl} > "$dir/niri.kdl"
    export NIRI_CONFIG="$dir/niri.kdl"

    # Where apps save things: new files land in Downloads, where Library
    # asks where they belong; the rest lives in Library's collections.
    if [ ! -e "$HOME/.config/user-dirs.dirs" ]; then
      mkdir -p "$HOME/.config" "$HOME/Downloads" \
        "$HOME/Library/Documents" "$HOME/Library/Photos" "$HOME/Library/Music" "$HOME/Library/Videos"
      cat > "$HOME/.config/user-dirs.dirs" <<'EOF'
    XDG_DESKTOP_DIR="$HOME/Downloads"
    XDG_DOWNLOAD_DIR="$HOME/Downloads"
    XDG_TEMPLATES_DIR="$HOME/Library/Documents"
    XDG_PUBLICSHARE_DIR="$HOME/Downloads"
    XDG_DOCUMENTS_DIR="$HOME/Library/Documents"
    XDG_MUSIC_DIR="$HOME/Library/Music"
    XDG_PICTURES_DIR="$HOME/Library/Photos"
    XDG_VIDEOS_DIR="$HOME/Library/Videos"
    EOF
    fi

    export XDG_CURRENT_DESKTOP=niri
    export XCURSOR_THEME=Adwaita
    export XCURSOR_SIZE=24
    export MOZ_ENABLE_WAYLAND=1
    export NIXOS_OZONE_WL=1
    exec ${config.programs.niri.package}/bin/niri-session
  '';

  # ---------------------------------------------------------------- login

  # Until the first run is done, the Chlorum first run (keyboard, network,
  # profile) is shown; afterwards the Chlorum login screen.
  firstRunUi = pkgs.runCommand "chlorum-ui" { nativeBuildInputs = [ pkgs.makeWrapper ]; } ''
    mkdir -p $out/share/chlorum-ui $out/bin
    cp -r ${../../ui}/. $out/share/chlorum-ui/
    chmod -R u+w $out/share/chlorum-ui
    cp ${../../assets/logo/chlorum-icon-small.svg} $out/share/chlorum-ui/logo.svg
    makeWrapper ${pkgs.python3}/bin/python3 $out/bin/chlorum-ui \
      --add-flags $out/share/chlorum-ui/chlorum-ui.py \
      --set CHLORUM_UI_DIR $out/share/chlorum-ui \
      --set CHLORUM_GREETER_GROUP greeter \
      --set CHLORUM_KIOSK_BROWSER librewolf \
      --prefix PATH : ${lib.makeBinPath [
        browser
        config.systemd.package
        config.networking.networkmanager.package
        pkgs.xkcdpass
        pkgs.kbd
        pkgs.getent
        pkgs.coreutils
      ]}
  '';
  greeter = pkgs.writeShellScript "chlorum-greeter" ''
    # cage draws at the screen's real size: scale 2 on 4K, like the session
    export COSMIC_SCALE=1
    for m in /sys/class/drm/card*-*/modes; do
      [ "$(cat "''${m%modes}status" 2>/dev/null)" = connected ] || continue
      h=$(head -n1 "$m" | cut -dx -f2)
      [ "''${h:-0}" -ge 2000 ] && export COSMIC_SCALE=2
      break
    done
    export XKB_DEFAULT_LAYOUT="$(cat /var/lib/chlorum/xkb-layout 2>/dev/null || echo us)"
    [ -e "${pkgs.xkeyboard_config}/share/X11/xkb/symbols/$XKB_DEFAULT_LAYOUT" ] || XKB_DEFAULT_LAYOUT=us
    export XDG_DATA_DIRS=/run/current-system/sw/share
    export XCURSOR_THEME=Adwaita
    if [ -e /var/lib/chlorum/setup-done ]; then
      export PATH=${lib.makeBinPath [ config.systemd.package pkgs.getent ]}:$PATH
      # The native login screen; if it cannot start three times in a row
      # (graphics trouble), the web login screen takes over.
      status="''${XDG_RUNTIME_DIR:-/tmp}/chlorum-greeter-status"
      for try in 1 2 3; do
        rm -f "$status"
        ${pkgs.cage}/bin/cage -s -d -- ${pkgs.bash}/bin/bash -c \
          '${chlorum}/bin/chlorum greeter; echo $? > "$1"' sh "$status"
        [ "$(cat "$status" 2>/dev/null)" = 0 ] && exit 0
        sleep 1
      done
    fi
    export MOZ_ENABLE_WAYLAND=1
    exec ${pkgs.cage}/bin/cage -s -d -- ${firstRunUi}/bin/chlorum-ui greeter
  '';

  # ---------------------------------------------------------------- boot

  # Big logo, the name, and a turning chlorine atom; asks for the disk's
  # recovery code here too if the security chip cannot unlock it.
  plymouthTheme = pkgs.runCommand "chlorum-plymouth" {
    nativeBuildInputs = [ pkgs.librsvg pkgs.imagemagick ];
  } ''
    d=$out/share/plymouth/themes/chlorum
    mkdir -p $d
    mono=$(find ${pkgs.ibm-plex}/share/fonts -iname 'IBMPlexMono-SemiBold.[ot]tf' | head -n1)
    rsvg-convert -h 860 ${../../assets/logo/chlorum-logo.svg} -o $d/logo.png
    magick -background none -fill '#FFD60A' -font "$mono" -pointsize 120 label:'C H L O R U M' PNG32:$d/word.png
    magick -size 400x400 xc:none -fill none -stroke '#333333' -strokewidth 4 -draw 'circle 200,200 200,4' PNG32:$d/ring.png
    magick -size 64x64 xc:none -fill '#FFD60A' -draw 'circle 32,32 32,2' PNG32:$d/electron.png
    magick -size 64x64 xc:none -fill none -stroke '#FFD60A' -strokewidth 5 -draw 'circle 32,32 32,5' PNG32:$d/hole.png
    magick -size 200x200 xc:none -fill '#FFD60A' -draw 'circle 100,100 100,2' \
      -fill black -font "$mono" -pointsize 96 -gravity center -annotate +0+0 'Cl' PNG32:$d/nucleus.png
    cp ${../../assets/plymouth/chlorum.script} $d/chlorum.script
    substitute ${../../assets/plymouth/chlorum.plymouth} $d/chlorum.plymouth --replace-fail @out@ $out
  '';
  plexSans = pkgs.runCommand "IBMPlexSans-Regular" { } ''
    cp "$(find ${pkgs.ibm-plex}/share/fonts -iname 'IBMPlexSans-Regular.[ot]tf' | head -n1)" $out
  '';
in
{
  options.chlorum.desktop.enable = lib.mkEnableOption ''
    the Chlorum desktop: niri with the Chlorum shell (Home, system menu, one
    or two apps on screen), live wallpapers, Library, graphical boot, first
    run and login
  '';

  config = lib.mkIf cfg.enable {
    # ---- Boot: big logo and the atom --------------------------------------
    boot.plymouth = {
      enable = true;
      theme = "chlorum";
      themePackages = [ plymouthTheme ];
      font = "${plexSans}";
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

    # ---- Session: niri + Chlorum shell -------------------------------------
    programs.niri = {
      enable = true;
      useNautilus = false;            # the plain file picker; Library is the file manager
    };
    hardware.graphics.enable = true;
    services.pipewire = {
      enable = true;
      alsa.enable = true;
      pulse.enable = true;
    };
    security.rtkit.enable = true;
    hardware.bluetooth.enable = lib.mkDefault true;
    services.upower.enable = true;
    security.polkit.enable = true;

    systemd.user.services.chlorum-shell = {
      description = "Chlorum shell: Home, system menu and app layouts";
      wantedBy = [ "graphical-session.target" ];
      partOf = [ "graphical-session.target" ];
      after = [ "graphical-session.target" ];
      # only in a Chlorum session (niri), not in a plain tty login
      unitConfig = {
        ConditionEnvironment = "NIRI_SOCKET";
        StartLimitIntervalSec = 0;
      };
      # the session's own PATH (imported by niri), not a minimal one
      enableDefaultPath = false;
      serviceConfig = {
        ExecStart = "${chlorum}/bin/chlorum shell";
        Restart = "always";
        RestartSec = 1;
      };
    };
    systemd.user.services.chlorum-wallpaper = {
      description = "Chlorum live wallpaper";
      wantedBy = [ "graphical-session.target" ];
      partOf = [ "graphical-session.target" ];
      after = [ "graphical-session.target" ];
      unitConfig.ConditionEnvironment = "NIRI_SOCKET";
      enableDefaultPath = false;
      serviceConfig = {
        ExecStart = "${wallpaper}/bin/chlorum-wallpaper";
        Restart = "on-failure";
        RestartSec = 2;
        Nice = 5;
      };
    };

    # ---- Login: greetd with the Chlorum login screen -----------------------
    services.greetd = {
      enable = true;
      settings.default_session = {
        command = "${greeter}";
        user = "greeter";
      };
    };
    users.users.greeter = {
      isSystemUser = true;
      home = "/var/lib/greeter";
      createHome = true;
      group = "greeter";
      extraGroups = [ "video" "input" ];
    };
    users.groups.greeter = { };
    # Encrypted profiles (systemd-homed) get user IDs 60001-60513
    security.loginDefs.settings.UID_MAX = 60513;
    systemd.services.greetd = {
      unitConfig.StartLimitIntervalSec = 0;
      serviceConfig = {
        Restart = lib.mkForce "always";
        RestartSec = 1;
      };
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
      environment.CHLORUM_GREETER_GROUP = "greeter";
      serviceConfig = {
        ExecStart = "${firstRunUi}/bin/chlorum-ui setupd";
        Restart = "on-failure";
      };
    };

    # Logging out ends everything the profile started, so its encrypted
    # volume can close.
    services.logind.settings.Login.KillUserProcesses = true;

    # ---- Apps ----------------------------------------------------------------
    environment.systemPackages = [
      chlorum
      wallpaper
      session
      browser
      cosmicDefaults
      pkgs.cosmic-term
      pkgs.cosmic-edit
      pkgs.papirus-icon-theme
      pkgs.adwaita-icon-theme
      pkgs.xwayland-satellite
      pkgs.xdg-utils
      pkgs.wl-clipboard
      pkgs.brightnessctl
      pkgs.git
      pkgs.gh
    ];
    xdg.mime.defaultApplications = {
      "text/html" = "librewolf.desktop";
      "x-scheme-handler/http" = "librewolf.desktop";
      "x-scheme-handler/https" = "librewolf.desktop";
      "inode/directory" = "chlorum-library.desktop";
    };
    fonts.packages = [ pkgs.ibm-plex pkgs.noto-fonts pkgs.noto-fonts-color-emoji ];
    fonts.fontconfig.defaultFonts = {
      sansSerif = [ "IBM Plex Sans" ];
      monospace = [ "IBM Plex Mono" ];
    };
  };
}
