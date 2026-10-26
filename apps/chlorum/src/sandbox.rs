//! What each app may use. Every app starts through `chlorum run`, which
//! reads the owner's choices and, when something is off, starts the app in
//! a bubblewrap sandbox that leaves it out:
//!
//!   network   its own empty network: no internet, no local network
//!   camera    the camera devices are not there (its own /dev)
//!   sound     no speakers and no microphone (no PipeWire, no ALSA)
//!   files     no personal files: Library, Downloads and the like are
//!             replaced by the app's own drop box, which Library shows under
//!             "Needs an answer". Files chosen for it in the picker are
//!             copied into that box.
//!
//! Any sandboxed app also gets: its own process list, no compositor control
//! socket, a session bus that only reaches the portals, a Wayland socket
//! with a security context (niri keeps typing-for-others, screen capture and
//! overlays away from it), and read-only login and Chlorum settings.
//!
//! Known gaps: apps opened by other apps (a link, a file type) start
//! without the sandbox; with only the camera off, a camera shared through
//! PipeWire is still reachable; X11 apps share one X server.
//!
//! The choices live in ~/.config/chlorum/permissions, one app per line:
//!   librewolf camera=off sound=off

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

    fn from_key(k: &str) -> Option<Perm> {
        Perm::ALL.into_iter().find(|p| p.key() == k)
    }
}

/// Everything off that the owner turned off; missing means on.
pub type Denied = HashMap<String, Vec<Perm>>;

fn store() -> PathBuf {
    crate::apps::config_dir().join("chlorum/permissions")
}

pub fn load() -> Denied {
    parse(&std::fs::read_to_string(store()).unwrap_or_default())
}

fn parse(s: &str) -> Denied {
    let mut out = Denied::new();
    for line in s.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.split_whitespace();
        let Some(app) = parts.next() else { continue };
        let off: Vec<Perm> = parts
            .filter_map(|kv| kv.split_once('='))
            .filter(|(_, v)| *v == "off")
            .filter_map(|(k, _)| Perm::from_key(k))
            .collect();
        if !off.is_empty() {
            out.insert(app.to_string(), off);
        }
    }
    out
}

fn render(d: &Denied) -> String {
    let mut apps: Vec<&String> = d.keys().collect();
    apps.sort();
    let mut s = String::from("# What apps may use. Changed in Center → Apps.\n");
    for a in apps {
        let off = &d[a];
        if off.is_empty() {
            continue;
        }
        s.push_str(a);
        for p in Perm::ALL {
            if off.contains(&p) {
                s.push_str(&format!(" {}=off", p.key()));
            }
        }
        s.push('\n');
    }
    s
}

pub fn allowed(d: &Denied, app: &str, p: Perm) -> bool {
    !d.get(app).is_some_and(|v| v.contains(&p))
}

pub fn set(app: &str, p: Perm, on: bool) -> std::io::Result<()> {
    let mut d = load();
    let v = d.entry(app.to_string()).or_default();
    v.retain(|x| *x != p);
    if !on {
        v.push(p);
    }
    let path = store();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("new");
    std::fs::write(&tmp, render(&d))?;
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

/// Folders with personal files that an app without "files" does not see.
const PERSONAL: &[&str] = &[
    "Library",
    "Downloads",
    "Desktop",
    "Documents",
    "Pictures",
    "Music",
    "Videos",
    "Templates",
    "Public",
    ".ssh",
    ".gnupg",
];
/// The same, under the data folder (~/.local/share).
const PERSONAL_DATA: &[&str] = &[
    "keyrings",
    "Trash",
    // every app's drop box: its own is bound back in below
    "chlorum",
];

/// What starts things at login or decides what apps may do: a sandboxed
/// app must not be able to change it (and so let itself out next time).
fn protected(home: &Path) -> Vec<PathBuf> {
    let config = crate::apps::config_dir();
    let data = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".local/share"));
    let mut v: Vec<PathBuf> = ["chlorum", "niri", "systemd", "autostart", "environment.d"].iter().map(|d| config.join(d)).collect();
    v.push(data.join("applications"));
    v.extend([".local/bin", ".bashrc", ".bash_profile", ".profile"].iter().map(|f| home.join(f)));
    v
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

/// The bubblewrap arguments for an app with `off` turned off.
pub fn bwrap_args(app: &str, off: &[Perm], runtime: &Path, home: &Path, devices: &[Dev], outside: &Outside) -> Vec<String> {
    let s = |p: &Path| p.to_string_lossy().to_string();
    let mut a: Vec<String> = vec!["--dev-bind".into(), "/".into(), "/".into()];
    // its own process list: it cannot reach other processes or their files
    // through /proc
    a.extend(["--unshare-pid", "--proc", "/proc", "--new-session"].map(String::from));

    // the compositor's control socket would let it start anything outside
    a.extend(["--unsetenv".into(), "NIRI_SOCKET".into()]);
    if let Some(n) = std::env::var_os("NIRI_SOCKET").filter(|n| Path::new(n).exists()) {
        a.extend(["--ro-bind".into(), "/dev/null".into(), n.to_string_lossy().to_string()]);
    }
    // the session bus would let it start anything too: only the filtered one
    let bus = runtime.join("bus");
    match &outside.bus {
        Some(p) => a.extend(["--ro-bind".into(), s(p), s(&bus)]),
        None if bus.exists() => a.extend(["--ro-bind".into(), "/dev/null".into(), s(&bus)]),
        None => {}
    }
    if let Some(w) = &outside.wayland {
        let display = std::env::var("WAYLAND_DISPLAY").unwrap_or_else(|_| "wayland-0".into());
        a.extend(["--ro-bind".into(), s(w), s(&runtime.join(display))]);
    }
    for p in protected(home) {
        if p.exists() {
            a.extend(["--ro-bind".into(), s(&p), s(&p)]);
        }
    }

    if off.contains(&Perm::Network) {
        a.push("--unshare-net".into());
    }
    let no_camera = off.contains(&Perm::Camera);
    let no_sound = off.contains(&Perm::Sound);
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
    if no_sound {
        for name in ["pipewire-0", "pipewire-0-manager", "pulse"] {
            let p = runtime.join(name);
            if p.is_dir() {
                a.extend(["--tmpfs".into(), s(&p)]);
            } else if p.exists() {
                a.extend(["--ro-bind".into(), "/dev/null".into(), s(&p)]);
            }
        }
    }
    if off.contains(&Perm::Files) {
        let data = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".local/share"));
        for p in PERSONAL.iter().map(|n| home.join(n)).chain(PERSONAL_DATA.iter().map(|n| data.join(n))) {
            if p.exists() {
                a.extend(["--tmpfs".into(), s(&p)]);
            }
        }
        // what it saves or downloads lands in its drop box
        let boxdir = drop_box(app);
        for name in ["Library", "Downloads"] {
            a.extend(["--bind".into(), s(&boxdir), s(&home.join(name))]);
        }
        for p in ["/run/media", "/media", "/mnt"] {
            if Path::new(p).exists() {
                a.extend(["--tmpfs".into(), p.into()]);
            }
        }
        a.extend(["--tmpfs".into(), "/tmp".into()]);
        if Path::new("/tmp/.X11-unix").exists() {
            a.extend(["--ro-bind".into(), "/tmp/.X11-unix".into(), "/tmp/.X11-unix".into()]);
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

/// `chlorum run <app-id> <command line>`: start an app with what it may use.
pub fn run_cli(args: &[String]) -> Result<(), String> {
    use std::os::unix::process::CommandExt;
    let app = args.first().ok_or("usage: chlorum run <app-id> <command>")?;
    let line = args[1..].join(" ");
    if line.trim().is_empty() {
        return Err("nothing to run".into());
    }
    let denied = load();
    let off: Vec<Perm> = Perm::ALL.into_iter().filter(|p| !allowed(&denied, app, *p)).collect();
    let mut cmd = if off.is_empty() {
        let mut c = std::process::Command::new("sh");
        c.arg("-c").arg(format!("exec {line}"));
        c
    } else {
        if off.contains(&Perm::Files) {
            std::fs::create_dir_all(drop_box(app)).map_err(|e| e.to_string())?;
        }
        let runtime = PathBuf::from(std::env::var("XDG_RUNTIME_DIR").unwrap_or_default());
        let devices: Vec<Dev> = std::fs::read_dir("/dev")
            .map(|rd| rd.flatten().map(|e| Dev { link: e.file_type().is_ok_and(|t| t.is_symlink()), path: e.path() }).collect())
            .unwrap_or_default();
        // the ends that keep the Wayland listener and the bus proxy alive
        // exactly as long as the sandbox
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
        let mut c = std::process::Command::new("bwrap");
        c.args(bwrap_args(app, &off, &runtime, &home(), &devices, &outside)).arg("--").arg("sh").arg("-c").arg(format!("exec {line}"));
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
        let d = parse("# c\nlibrewolf camera=off sound=off network=on\nfoo files=off\n\nbar network=on\n");
        assert_eq!(d["librewolf"], vec![Perm::Camera, Perm::Sound]);
        assert_eq!(d["foo"], vec![Perm::Files]);
        assert!(!d.contains_key("bar"));
        assert!(allowed(&d, "bar", Perm::Network));
        assert!(!allowed(&d, "librewolf", Perm::Camera));
        assert_eq!(parse(&render(&d)), d);
    }

    #[test]
    fn sandbox_args() {
        let dev = |p: &str, link| Dev { path: PathBuf::from(p), link };
        let devs = vec![dev("/dev/video0", false), dev("/dev/sda", false), dev("/dev/media1", false), dev("/dev/snd", false), dev("/dev/null", false), dev("/dev/cdrom", true), dev("/dev/dri", false)];
        let a = bwrap_args("x", &[Perm::Network, Perm::Camera], Path::new("/nonexistent"), Path::new("/nonexistent"), &devs, &Outside::default());
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
        let a = bwrap_args("x", &[Perm::Sound], Path::new("/nonexistent"), Path::new("/nonexistent"), &devs, &Outside::default()).join(" ");
        assert!(a.contains("/dev/video0"));
        assert!(!a.contains("/dev/snd"));
    }
}
