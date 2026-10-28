//! What each app may use. Every app starts through `chlorum run`, which
//! reads the owner's choices and, when something is off, starts the app in
//! a bubblewrap sandbox that leaves it out.
//!
//! Nothing is allowed until the owner says so. The first time an app
//! starts, a window like a phone's asks about each thing it wants (see
//! `needs` and ask.rs): Allow or Don't allow. What it did not ask for stays
//! off; Center → Apps changes any of it later. Programs made in Workshop
//! follow the same rules when they run (`work run`, `work test`).
//!
//!   network   its own empty network: no internet, no local network
//!   camera    the camera devices are not there (its own /dev)
//!   sound     no speakers and no microphone (no PipeWire, no ALSA)
//!   files     a home folder of its own instead of yours (kept between
//!             starts; your files, settings and other apps' data are not in
//!             it, only themes and fonts, read-only). Its Library and
//!             Downloads are its drop box, which Library shows under "Needs
//!             an answer". Files chosen for it in the picker are copied into
//!             that box; a file opened with it from Library is let in, that
//!             file alone, under /tmp/chlorum-open (`--grant`).
//!
//! Any sandboxed app also gets: its own process list; an empty runtime
//! folder ($XDG_RUNTIME_DIR) with only a Wayland socket with a security
//! context (niri keeps typing-for-others, screen capture and overlays away
//! from it), a session bus that only reaches the portals, and PipeWire when
//! it may use sound (so no compositor control, no systemd, no keyboard
//! mouse, no ssh or gpg agent); and read-only login and Chlorum settings.
//!
//! Known gaps: an app started by an app that may do everything (from a
//! terminal, say) may do everything too; with only the camera off, a camera
//! shared through PipeWire is still reachable; X11 apps share one X server;
//! the Nix daemon is reachable, and it can download for a build even when
//! the app itself has no network.
//!
//! The choices live in ~/.config/chlorum/permissions, one app per line;
//! what is not written there has not been asked yet, and is off:
//!   librewolf network=on sound=on camera=off

use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Perm {
    Network,
    Camera,
    Sound,
    Files,
}

impl Perm {
    pub const ALL: [Perm; 4] = [Perm::Network, Perm::Camera, Perm::Sound, Perm::Files];

    pub fn key(self) -> &'static str {
        match self {
            Perm::Network => "network",
            Perm::Camera => "camera",
            Perm::Sound => "sound",
            Perm::Files => "files",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Perm::Network => "Internet and network",
            Perm::Camera => "Camera",
            Perm::Sound => "Sound and microphone",
            Perm::Files => "Personal files",
        }
    }

    pub fn off_means(self) -> &'static str {
        match self {
            Perm::Network => "No internet, no local network.",
            Perm::Camera => "The camera is not there for it.",
            Perm::Sound => "It cannot play sound or hear the microphone.",
            Perm::Files => "It sees only the files you choose for it.",
        }
    }

    /// The question in the window that asks, for an app called `name`.
    pub fn question(self, name: &str) -> String {
        match self {
            Perm::Network => format!("Allow {name} to use the internet and your network?"),
            Perm::Camera => format!("Allow {name} to use the camera?"),
            Perm::Sound => format!("Allow {name} to play sound and use the microphone?"),
            Perm::Files => format!("Allow {name} to see your personal files?"),
        }
    }

    pub fn from_key(k: &str) -> Option<Perm> {
        Perm::ALL.into_iter().find(|p| p.key() == k)
    }
}

/// The owner's answers: app → (permission → allowed). Not there: not
/// asked yet, which counts as not allowed.
pub type Choices = HashMap<String, HashMap<Perm, bool>>;

/// Apps that are part of the system itself and start as they are: the
/// Library is where files are chosen, so it has to see them; Terminal and
/// Typewriter (with its terminal) are where the system is looked after and
/// programs are made, and they start other apps, which then ask for
/// themselves.
pub const SYSTEM: &[&str] = &["chlorum-library", "chlorum-gallery", "com.system76.CosmicTerm", "chlorum-typewriter"];

/// What an app asks for the first time it starts. Apps may say it in their
/// .desktop entry (`X-Chlorum-Permissions=network;sound;`); for the ones
/// Chlorum ships it is known; anything else asks about everything.
pub fn needs(app: &str, declared: Option<&str>) -> Vec<Perm> {
    if let Some(d) = declared {
        return d.split([';', ',', ' ']).filter_map(|k| Perm::from_key(k.trim())).collect();
    }
    // installed from Center → Get apps: the catalog says
    if let Some(n) = crate::store::needs_of(app) {
        return n;
    }
    match app {
        "librewolf" | "firefox" | "chromium" => vec![Perm::Network, Perm::Sound, Perm::Camera],
        "mpv" => vec![Perm::Sound],
        "org.gnome.SimpleScan" | "simple-scan" => vec![Perm::Network],
        _ => Perm::ALL.to_vec(),
    }
}

fn store() -> PathBuf {
    crate::apps::config_dir().join("chlorum/permissions")
}

pub fn load() -> Choices {
    parse(&std::fs::read_to_string(store()).unwrap_or_default())
}

fn parse(s: &str) -> Choices {
    let mut out = Choices::new();
    for line in s.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.split_whitespace();
        let Some(app) = parts.next() else { continue };
        let set: HashMap<Perm, bool> = parts
            .filter_map(|kv| kv.split_once('='))
            .filter_map(|(k, v)| Some((Perm::from_key(k)?, match v { "on" => true, "off" => false, _ => return None })))
            .collect();
        if !set.is_empty() {
            out.entry(app.to_string()).or_default().extend(set);
        }
    }
    out
}

fn render(c: &Choices) -> String {
    let mut apps: Vec<&String> = c.keys().collect();
    apps.sort();
    let mut s = String::from("# What apps may use: on, off; not written = not asked yet (off). Changed in Center → Apps.\n");
    for a in apps {
        let set = &c[a];
        if set.is_empty() {
            continue;
        }
        s.push_str(a);
        for p in Perm::ALL {
            if let Some(on) = set.get(&p) {
                s.push_str(&format!(" {}={}", p.key(), if *on { "on" } else { "off" }));
            }
        }
        s.push('\n');
    }
    s
}

pub fn allowed(c: &Choices, app: &str, p: Perm) -> bool {
    SYSTEM.contains(&app) || c.get(app).and_then(|m| m.get(&p)).copied().unwrap_or(false)
}

/// Has the owner answered about this one?
pub fn answered(c: &Choices, app: &str, p: Perm) -> bool {
    c.get(app).is_some_and(|m| m.contains_key(&p))
}

pub fn set(app: &str, p: Perm, on: bool) -> std::io::Result<()> {
    set_many(app, &[(p, on)])
}

pub fn set_many(app: &str, answers: &[(Perm, bool)]) -> std::io::Result<()> {
    let mut c = load();
    let m = c.entry(app.to_string()).or_default();
    for (p, on) in answers {
        m.insert(*p, *on);
    }
    let path = store();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("new");
    std::fs::write(&tmp, render(&c))?;
    std::fs::rename(tmp, path)
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_default())
}

/// Where an app without personal files puts and gets files.
pub fn drop_box(app: &str) -> PathBuf {
    let data = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".local/share"));
    data.join("chlorum/drop").join(app.replace('/', "_"))
}

/// All drop boxes, for Library's "Needs an answer".
pub fn drop_boxes() -> Vec<PathBuf> {
    let root = drop_box("x").parent().map(Path::to_path_buf).unwrap_or_default();
    std::fs::read_dir(root).map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect()).unwrap_or_default()
}

fn data_home(home: &Path) -> PathBuf {
    std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".local/share"))
}

/// The home folder an app without "files" gets instead of yours: its own,
/// kept between starts. Your files, your other apps' data and your
/// settings are not in it.
pub fn private_home(app: &str) -> PathBuf {
    data_home(&home()).join("chlorum/homes").join(app.replace('/', "_"))
}

/// Read-only from your home into a private one, so apps still look right:
/// themes, fonts, cursor.
const LOOKS: &[&str] = &[
    ".config/cosmic",
    ".config/gtk-3.0",
    ".config/gtk-4.0",
    ".config/fontconfig",
    ".local/share/fonts",
    ".icons",
    ".fonts",
];

/// An app's own data, kept where it always was even when it may not see
/// your files: the browser keeps its profile (bookmarks, sign-ins).
fn own_dirs(app: &str) -> &'static [&'static str] {
    match app {
        "librewolf" => &[".librewolf", ".cache/librewolf"],
        "firefox" => &[".mozilla", ".cache/mozilla"],
        _ => &[],
    }
}

/// What starts things at login or decides what apps may do: a sandboxed
/// app must not be able to change it (and so let itself out next time).
fn protected(home: &Path) -> Vec<PathBuf> {
    let config = crate::apps::config_dir();
    let data = data_home(home);
    let mut v: Vec<PathBuf> = ["chlorum", "niri", "systemd", "autostart", "environment.d"].iter().map(|d| config.join(d)).collect();
    v.push(data.join("applications"));
    v.push(data.join("systemd"));
    v.extend([".local/bin", ".bashrc", ".bash_profile", ".profile"].iter().map(|f| home.join(f)));
    v
}

/// Makes every protected place exist, so that it can be made read-only: a
/// missing one could otherwise be created by the app.
pub fn ensure_protected(home: &Path) {
    for p in protected(home) {
        if p.exists() {
            continue;
        }
        let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let body = match name.as_str() {
            // the same as having none: bash then reads ~/.profile
            ".bash_profile" => Some("# Made by Chlorum so that apps cannot add one. Same as having none.\n[ -f ~/.profile ] && . ~/.profile\n"),
            ".bashrc" | ".profile" => Some(""),
            _ => None,
        };
        let _ = match body {
            Some(b) => std::fs::write(&p, b),
            None => std::fs::create_dir_all(&p),
        };
    }
}

/// /dev entries bubblewrap makes itself with --dev.
const DEV_OWN: &[&str] = &["null", "zero", "full", "random", "urandom", "tty", "pts", "ptmx", "shm", "console", "fd", "stdin", "stdout", "stderr", "core"];

/// What `chlorum run` prepared outside the sandbox and binds into it.
#[derive(Default)]
pub struct Outside {
    /// a Wayland socket whose clients niri treats as sandboxed (no virtual
    /// keyboard or pointer, no screen capture, no layer surfaces)
    pub wayland: Option<PathBuf>,
    /// a session-bus socket that lets through only the portals and
    /// notifications (xdg-dbus-proxy)
    pub bus: Option<PathBuf>,
}

/// One /dev entry as the sandbox should see it.
pub struct Dev {
    pub path: PathBuf,
    pub link: bool,
}

/// Where a file opened with an app that may not see your files is shown
/// to it: in its own /tmp, so nothing is left behind anywhere.
pub fn grant_place(i: usize, src: &Path) -> PathBuf {
    let name = src.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "file".into());
    PathBuf::from(format!("/tmp/chlorum-open/{i}/{name}"))
}

/// The bubblewrap arguments for an app with `off` turned off. `grants` are
/// (file, where the app sees it).
pub fn bwrap_args(app: &str, off: &[Perm], runtime: &Path, home: &Path, devices: &[Dev], outside: &Outside, grants: &[(PathBuf, PathBuf)]) -> Vec<String> {
    let s = |p: &Path| p.to_string_lossy().to_string();
    let mut a: Vec<String> = vec!["--dev-bind".into(), "/".into(), "/".into()];
    // its own process list: it cannot reach other processes or their files
    // through /proc
    a.extend(["--unshare-pid", "--proc", "/proc", "--new-session"].map(String::from));
    a.extend(["--setenv", "CHLORUM_SANDBOXED", "1", "--unsetenv", "NIRI_SOCKET"].map(String::from));
    let no_sound = off.contains(&Perm::Sound);

    // The session's runtime folder holds the keys to everything (the
    // compositor, the session bus, systemd, the keyboard mouse, ssh and gpg
    // agents): an empty one, with back only what it may use.
    if !runtime.as_os_str().is_empty() && runtime.is_dir() {
        a.extend(["--perms".into(), "0700".into(), "--tmpfs".into(), s(runtime)]);
        let display = std::env::var("WAYLAND_DISPLAY").unwrap_or_else(|_| "wayland-0".into());
        let wl = runtime.join(&display);
        match &outside.wayland {
            Some(w) => a.extend(["--ro-bind".into(), s(w), s(&wl)]),
            // no sandboxed socket (an old compositor): the normal one
            None if wl.exists() => a.extend(["--ro-bind".into(), s(&wl), s(&wl)]),
            None => {}
        }
        if let Some(p) = &outside.bus {
            a.extend(["--ro-bind".into(), s(p), s(&runtime.join("bus"))]);
        }
        if !no_sound {
            for name in ["pipewire-0", "pulse"] {
                let p = runtime.join(name);
                if p.exists() {
                    a.extend(["--ro-bind".into(), s(&p), s(&p)]);
                }
            }
        }
    }
    for p in protected(home) {
        if p.exists() {
            a.extend(["--ro-bind".into(), s(&p), s(&p)]);
        }
    }
    // the security-key helper refuses sandboxed apps anyway; they do not
    // even see it
    if Path::new("/run/chlorum-keyd").is_dir() {
        a.extend(["--tmpfs".into(), "/run/chlorum-keyd".into()]);
    }

    if off.contains(&Perm::Network) {
        a.push("--unshare-net".into());
    }
    let no_camera = off.contains(&Perm::Camera);
    if no_camera || no_sound {
        // a /dev of its own with only what it may use: a camera plugged in
        // later does not appear either
        a.extend(["--dev".into(), "/dev".into()]);
        for d in devices {
            let name = d.path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            let camera = name.starts_with("video") || name.starts_with("media") || name == "v4l";
            let sound = name == "snd";
            if d.link || DEV_OWN.contains(&name.as_str()) || (no_camera && camera) || (no_sound && sound) {
                continue;
            }
            a.extend(["--dev-bind".into(), s(&d.path), s(&d.path)]);
        }
    }
    if off.contains(&Perm::Files) {
        // no drives, no shared /tmp (first: a home could be under these)
        for p in ["/run/media", "/media", "/mnt"] {
            if Path::new(p).exists() {
                a.extend(["--tmpfs".into(), p.into()]);
            }
        }
        a.extend(["--tmpfs".into(), "/tmp".into()]);
        if Path::new("/tmp/.X11-unix").exists() {
            a.extend(["--ro-bind".into(), "/tmp/.X11-unix".into(), "/tmp/.X11-unix".into()]);
        }
        // a home of its own: none of your files, settings or other apps' data
        a.extend(["--bind".into(), s(&private_home(app)), s(home)]);
        for rel in LOOKS {
            let p = home.join(rel);
            if p.exists() {
                a.extend(["--ro-bind".into(), s(&p), s(&p)]);
            }
        }
        for rel in own_dirs(app) {
            let p = home.join(rel);
            if p.is_dir() {
                a.extend(["--bind".into(), s(&p), s(&p)]);
            }
        }
        // what it saves or downloads lands in its drop box
        let boxdir = drop_box(app);
        for name in ["Library", "Downloads"] {
            a.extend(["--bind".into(), s(&boxdir), s(&home.join(name))]);
        }
        // what the owner opened with it (from Library): that, and only that
        for (src, dest) in grants {
            if src.exists() && !protected(home).iter().any(|p| src.starts_with(p)) {
                a.extend(["--bind".into(), s(src), s(dest)]);
            }
        }
    }
    a
}

/// Keeps `fd` open across exec (Rust closes everything else).
fn inherit(fd: &impl std::os::fd::AsRawFd) {
    // SAFETY: plain fcntl on a descriptor we own
    unsafe {
        let raw = fd.as_raw_fd();
        let flags = libc::fcntl(raw, libc::F_GETFD);
        libc::fcntl(raw, libc::F_SETFD, flags & !libc::FD_CLOEXEC);
    }
}

/// A Wayland socket for the sandboxed app with a security context: niri
/// keeps the restricted protocols (typing or clicking for others, screen
/// capture, overlays) away from it. `keep` must stay open while the app
/// runs; when it closes, niri stops listening.
fn wayland_context(app: &str) -> Result<(PathBuf, std::os::unix::net::UnixStream), String> {
    use std::os::fd::AsFd;
    use wayland_client::protocol::wl_registry;
    use wayland_client::{Connection, Dispatch, QueueHandle, globals::GlobalListContents, globals::registry_queue_init};
    use wayland_protocols::wp::security_context::v1::client::{wp_security_context_manager_v1::WpSecurityContextManagerV1, wp_security_context_v1::WpSecurityContextV1};
    struct S;
    impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for S {
        fn event(_: &mut S, _: &wl_registry::WlRegistry, _: wl_registry::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<S>) {}
    }
    impl Dispatch<WpSecurityContextManagerV1, ()> for S {
        fn event(_: &mut S, _: &WpSecurityContextManagerV1, _: <WpSecurityContextManagerV1 as wayland_client::Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<S>) {}
    }
    impl Dispatch<WpSecurityContextV1, ()> for S {
        fn event(_: &mut S, _: &WpSecurityContextV1, _: <WpSecurityContextV1 as wayland_client::Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<S>) {}
    }
    let conn = Connection::connect_to_env().map_err(|e| e.to_string())?;
    let (globals, mut queue) = registry_queue_init::<S>(&conn).map_err(|e| e.to_string())?;
    let qh = queue.handle();
    let manager: WpSecurityContextManagerV1 = globals.bind(&qh, 1..=1, ()).map_err(|_| "no security-context in this compositor".to_string())?;
    let dir = crate::apps::runtime_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    // short: a socket path has a length limit
    let path = dir.join(format!("wl-{}", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let listener = std::os::unix::net::UnixListener::bind(&path).map_err(|e| e.to_string())?;
    let (keep, give) = std::os::unix::net::UnixStream::pair().map_err(|e| e.to_string())?;
    let ctx = manager.create_listener(listener.as_fd(), give.as_fd(), &qh, ());
    ctx.set_sandbox_engine("org.chlorum.run".into());
    ctx.set_app_id(app.into());
    ctx.set_instance_id(std::process::id().to_string());
    ctx.commit();
    queue.roundtrip(&mut S).map_err(|e| e.to_string())?;
    // niri holds its own copies of both now
    drop(listener);
    drop(give);
    Ok((path, keep))
}

/// The session bus as the sandboxed app sees it: portals (open/save,
/// notifications, settings) and its own name, nothing else.
fn bus_proxy(app: &str, keep: &std::os::unix::net::UnixStream) -> Option<PathBuf> {
    use std::os::fd::AsRawFd;
    use std::os::unix::process::CommandExt;
    let addr = std::env::var("DBUS_SESSION_BUS_ADDRESS").ok()?;
    let path = crate::apps::runtime_dir().join(format!("bus-{}", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let fd = keep.as_raw_fd();
    let mut c = std::process::Command::new("xdg-dbus-proxy");
    c.arg(&addr)
        .arg(&path)
        .arg("--filter")
        .arg("--talk=org.freedesktop.portal.*")
        .arg("--talk=org.freedesktop.Notifications")
        .arg("--talk=org.a11y.Bus")
        // saved passwords (the keyring), as apps expect it
        .arg("--talk=org.freedesktop.secrets")
        // its own well-known name, when its id is one (reverse-DNS)
        // the proxy quits when the sandbox closes its end
        .arg(format!("--fd={fd}"));
    if app.contains('.') && app.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c)) {
        c.arg(format!("--own={app}")).arg(format!("--own={app}.*"));
    }
    // SAFETY: only an fcntl between fork and exec
    unsafe {
        c.pre_exec(move || {
            let flags = libc::fcntl(fd, libc::F_GETFD);
            libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC);
            Ok(())
        });
    }
    c.spawn().ok()?;
    for _ in 0..60 {
        if path.exists() {
            return Some(path);
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    None
}

/// What the owner allows `app` now. The first time, a window asks about
/// each thing it wants and the answers are kept; when that window cannot
/// be shown, what has not been answered stays off (and is asked next time).
pub fn resolve(app: &str, name: &str, icon: &str, wants: &[Perm]) -> Vec<Perm> {
    let mut choices = load();
    // inside another app's sandbox nobody can be asked (and nothing saved):
    // what has not been answered stays off
    if std::env::var_os("CHLORUM_SANDBOXED").is_some() {
        if wants.iter().any(|p| !answered(&choices, app, *p)) {
            eprintln!("chlorum: {app} has not been allowed what it asks for yet. Start it from Home (or a terminal outside any app) to be asked, or choose in Center → Apps.");
        }
        return Perm::ALL.into_iter().filter(|p| !allowed(&choices, app, *p)).collect();
    }
    let unanswered = wants.iter().any(|p| !answered(&choices, app, *p));
    // one question at a time: wait for any other, then look again (it may
    // have been about this same app)
    let _turn = if unanswered && !SYSTEM.contains(&app) { take_turn() } else { None };
    if _turn.is_some() {
        choices = load();
    }
    let ask: Vec<Perm> = wants.iter().copied().filter(|p| !answered(&choices, app, *p)).collect();
    if !ask.is_empty() && !SYSTEM.contains(&app) {
        match crate::ask::ask(app, name, icon, &ask) {
            Ok(answers) => {
                if let Err(e) = set_many(app, &answers) {
                    eprintln!("chlorum: {app}: could not keep the answers: {e}");
                }
                choices = load();
                for (p, on) in answers {
                    choices.entry(app.to_string()).or_default().insert(p, on);
                }
            }
            Err(e) => eprintln!("chlorum: {app}: could not ask ({e}); starting with nothing new allowed"),
        }
    }
    Perm::ALL.into_iter().filter(|p| !allowed(&choices, app, *p)).collect()
}

fn ask_lock() -> Option<std::fs::File> {
    let dir = crate::apps::runtime_dir();
    std::fs::create_dir_all(&dir).ok()?;
    std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(dir.join("ask.lock")).ok()
}

/// Holds the lock that lets one question be asked at a time (until
/// dropped). The kernel lets go of it when the process ends, however.
fn take_turn() -> Option<std::fs::File> {
    use std::os::fd::AsRawFd;
    let f = ask_lock()?;
    // SAFETY: flock on a descriptor we own
    unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX) };
    Some(f)
}

/// Is a question on screen right now? (Someone holds the lock.) The shell
/// then gives up the keyboard. Sandboxed apps cannot reach the lock.
pub fn asking() -> bool {
    use std::os::fd::AsRawFd;
    let Some(f) = ask_lock() else { return false };
    // SAFETY: flock on a descriptor we own; closing the file lets go
    let free = unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0;
    !free
}

/// Everything a sandbox needs from outside: the /dev list, a sandboxed
/// Wayland socket and a filtered session bus. The sockets stay alive as
/// long as this process and what it becomes (exec) live.
pub fn prepare(app: &str) -> (Vec<Dev>, Outside) {
    let devices: Vec<Dev> = std::fs::read_dir("/dev")
        .map(|rd| rd.flatten().map(|e| Dev { link: e.file_type().is_ok_and(|t| t.is_symlink()), path: e.path() }).collect())
        .unwrap_or_default();
    let mut outside = Outside::default();
    let mut keep = Vec::new();
    match wayland_context(app) {
        Ok((path, k)) => {
            outside.wayland = Some(path);
            keep.push(k);
        }
        Err(e) => eprintln!("chlorum: {app}: no sandboxed Wayland socket ({e}); using the normal one"),
    }
    if let Ok((mine, theirs)) = std::os::unix::net::UnixStream::pair() {
        outside.bus = bus_proxy(app, &theirs);
        drop(theirs);
        keep.push(mine);
    }
    if outside.bus.is_none() {
        eprintln!("chlorum: {app}: no filtered session bus; it gets none");
    }
    for k in &keep {
        inherit(k);
    }
    std::mem::forget(keep);
    (devices, outside)
}

pub fn runtime() -> PathBuf {
    PathBuf::from(std::env::var("XDG_RUNTIME_DIR").unwrap_or_default())
}

/// `chlorum run <app-id> [--grant <path>]... <command line>`: start an app
/// with what it may use. `--grant` lets one file or folder in, for an app
/// that may not see personal files (Library uses it to open a file).
pub fn run_cli(args: &[String]) -> Result<(), String> {
    use std::os::unix::process::CommandExt;
    let app = args.first().ok_or("usage: chlorum run <app-id> [--grant <path>] <command>")?;
    let mut rest = &args[1..];
    // (as written on the command line, the file itself)
    let mut grants: Vec<(String, PathBuf)> = Vec::new();
    while rest.first().map(String::as_str) == Some("--grant") {
        let p = rest.get(1).ok_or("--grant needs a path")?;
        grants.push((p.clone(), std::fs::canonicalize(p).unwrap_or_else(|_| PathBuf::from(p))));
        rest = &rest[2..];
    }
    let mut line = rest.join(" ");
    if line.trim().is_empty() {
        return Err("nothing to run".into());
    }
    let entry = crate::apps::installed().remove(app.as_str());
    let name = entry.as_ref().map(|a| a.name.clone()).unwrap_or_else(|| app.clone());
    let icon = entry.as_ref().map(|a| a.icon.clone()).unwrap_or_else(|| "application-x-executable".into());
    let wants = needs(app, entry.as_ref().and_then(|a| a.permissions.as_deref()));
    let off = resolve(app, &name, &icon, &wants);
    let mut cmd = if off.is_empty() {
        let mut c = std::process::Command::new("sh");
        c.arg("-c").arg(format!("exec {line}"));
        c
    } else {
        let home = home();
        ensure_protected(&home);
        let mut places = Vec::new();
        if off.contains(&Perm::Files) {
            std::fs::create_dir_all(drop_box(app)).map_err(|e| e.to_string())?;
            std::fs::create_dir_all(private_home(app)).map_err(|e| e.to_string())?;
            for rel in own_dirs(app) {
                let _ = std::fs::create_dir_all(home.join(rel));
            }
            // the app is told where it sees each opened file
            for (i, (written, file)) in grants.iter().enumerate() {
                let place = grant_place(i, file);
                line = line.replace(&crate::apps::quote(written), &crate::apps::quote(&place.to_string_lossy()));
                places.push((file.clone(), place));
            }
        }
        let (devices, outside) = prepare(app);
        let mut c = std::process::Command::new("bwrap");
        c.args(bwrap_args(app, &off, &runtime(), &home, &devices, &outside, &places)).arg("--").arg("sh").arg("-c").arg(format!("exec {line}"));
        c
    };
    cmd.env("CHLORUM_APP", app);
    if off.contains(&Perm::Files) {
        cmd.env("CHLORUM_NO_FILES", "1");
    }
    Err(format!("could not start {app}: {}", cmd.exec()))
}

/// Which app has the focused window, and whether it may see personal files.
/// Read from the environment `chlorum run` gave the app's process.
pub async fn focused_app() -> Option<(String, bool)> {
    let resp = crate::niri::send(crate::niri::Request::FocusedWindow).await.ok()?;
    let crate::niri::Response::FocusedWindow(Some(w)) = resp else { return None };
    let env = std::fs::read(format!("/proc/{}/environ", w.pid?)).ok()?;
    let mut app = None;
    let mut no_files = false;
    for kv in env.split(|b| *b == 0) {
        let kv = String::from_utf8_lossy(kv);
        if let Some(v) = kv.strip_prefix("CHLORUM_APP=") {
            app = Some(v.to_string());
        }
        if kv == "CHLORUM_NO_FILES=1" {
            no_files = true;
        }
    }
    app.map(|a| (a, !no_files))
}

/// Inside its drop box, where handed-over files go. Library does not ask
/// about these: they are copies of files already filed.
const HANDED: &str = "From Library";

/// For an app without personal files, chosen files are copied into its
/// drop box (the app sees them under ~/Library); a chosen collection
/// becomes the drop box itself. Others get the paths as they are.
pub fn hand_over(app: Option<&(String, bool)>, paths: Vec<PathBuf>, saving: bool) -> Vec<PathBuf> {
    let Some((app, false)) = app else { return paths };
    let boxdir = drop_box(app);
    let handed = boxdir.join(HANDED);
    let _ = std::fs::create_dir_all(&handed);
    let inside = home().join("Library");
    paths
        .into_iter()
        .filter_map(|p| {
            if p.is_dir() {
                return Some(inside.clone());
            }
            let name = p.file_name()?.to_string_lossy().to_string();
            if saving {
                return Some(inside.join(name));
            }
            let mut target = handed.join(&name);
            let mut n = 2;
            while target.exists() {
                target = handed.join(format!("{n}-{name}"));
                n += 1;
            }
            // a copy: the app cannot change the owner's original
            std::fs::copy(&p, &target).ok()?;
            Some(inside.join(HANDED).join(target.file_name()?))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_round_trip() {
        let d = parse("# c\nlibrewolf camera=off sound=off network=on\nfoo files=off\n\nbar network=maybe\n");
        assert!(allowed(&d, "librewolf", Perm::Network));
        assert!(!allowed(&d, "librewolf", Perm::Camera));
        assert!(answered(&d, "librewolf", Perm::Sound));
        // never asked: not allowed
        assert!(!answered(&d, "librewolf", Perm::Files));
        assert!(!allowed(&d, "librewolf", Perm::Files));
        assert!(!allowed(&d, "nobody", Perm::Network));
        assert!(!d.contains_key("bar"));
        assert!(allowed(&d, "chlorum-library", Perm::Files));
        assert_eq!(parse(&render(&d)), d);
    }

    #[test]
    fn what_apps_ask_for() {
        assert_eq!(needs("librewolf", None), vec![Perm::Network, Perm::Sound, Perm::Camera]);
        assert_eq!(needs("x", Some("network; files")), vec![Perm::Network, Perm::Files]);
        assert_eq!(needs("unknown", None).len(), 4);
    }

    #[test]
    fn sandbox_args() {
        let dev = |p: &str, link| Dev { path: PathBuf::from(p), link };
        let devs = vec![dev("/dev/video0", false), dev("/dev/sda", false), dev("/dev/media1", false), dev("/dev/snd", false), dev("/dev/null", false), dev("/dev/cdrom", true), dev("/dev/dri", false)];
        let a = bwrap_args("x", &[Perm::Network, Perm::Camera], Path::new("/nonexistent"), Path::new("/nonexistent"), &devs, &Outside::default(), &[]);
        let j = a.join(" ");
        assert!(j.starts_with("--dev-bind / / --unshare-pid --proc /proc"));
        assert!(j.contains("--unsetenv NIRI_SOCKET"));
        assert!(j.contains("--unshare-net"));
        assert!(j.contains("--dev /dev"));
        assert!(j.contains("--dev-bind /dev/dri /dev/dri"));
        assert!(j.contains("--dev-bind /dev/snd /dev/snd"));
        assert!(!j.contains("video0"));
        assert!(!j.contains("media1"));
        assert!(!j.contains("cdrom"));
        assert!(!j.contains("--dev-bind /dev/null"));
        let a = bwrap_args("x", &[Perm::Sound], Path::new("/nonexistent"), Path::new("/nonexistent"), &devs, &Outside::default(), &[]).join(" ");
        assert!(a.contains("/dev/video0"));
        assert!(!a.contains("/dev/snd"));
    }

    #[test]
    fn runtime_and_files() {
        let tmp = std::env::temp_dir().join(format!("chlorum-test-{}", std::process::id()));
        let run = tmp.join("run");
        std::fs::create_dir_all(run.join("pulse")).unwrap();
        std::fs::write(run.join("pipewire-0"), "").unwrap();
        std::fs::write(run.join("bus"), "").unwrap();
        let home = tmp.join("home");
        let file = home.join("Library/a b.txt");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, "x").unwrap();
        let out = Outside { wayland: Some(PathBuf::from("/x/wl-1")), bus: Some(PathBuf::from("/x/bus-1")) };
        let place = grant_place(0, &file);
        let j = bwrap_args("app", &[Perm::Files, Perm::Sound], &run, &home, &[], &out, &[(file.clone(), place.clone())]).join(" ");
        let r = run.to_string_lossy();
        // an empty runtime folder with only the sandboxed sockets
        assert!(j.contains(&format!("--tmpfs {r}")));
        assert!(j.contains(&format!("--ro-bind /x/bus-1 {r}/bus")));
        assert!(j.contains("--ro-bind /x/wl-1"));
        assert!(!j.contains("pipewire-0"), "no sound: no PipeWire");
        // its own home, the opened file in its /tmp
        assert!(j.contains(&format!("--bind {} {}", private_home("app").display(), home.display())));
        assert_eq!(place, PathBuf::from("/tmp/chlorum-open/0/a b.txt"));
        assert!(j.contains(&format!("--bind {} /tmp/chlorum-open/0/a b.txt", file.display())));
        let j = bwrap_args("app", &[Perm::Network], &run, &home, &[], &out, &[]).join(" ");
        assert!(j.contains("pipewire-0") && j.contains("pulse"));
        assert!(!j.contains("chlorum/homes"), "files allowed: the real home");
        ensure_protected(&home);
        assert!(home.join(".bash_profile").is_file());
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
