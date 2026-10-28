//! `chlorum keyd` — opens encrypted flash drives with a security key
//! (YubiKey or any FIDO2 key). Runs as root, because setting up an
//! encrypted drive with a key is something only root may do; Library asks
//! it over a socket, one JSON request per connection:
//!
//!   {"op":"unlock", "device":"/dev/sdb1", "pin":"…"}           open it with the key
//!   {"op":"enroll", "device":"/dev/sdb1", "password":"…", "pin":"…"}
//!                                                               add the key to it
//!   {"op":"lock",   "device":"/dev/sdb1"}                       close what unlock opened
//!
//! It only ever touches removable LUKS2 drives (never the computer's own
//! disk), and only answers programs that are not in an app sandbox. The
//! password and PIN are never written to disk or logged: they live in its
//! memory and, while the one command that needs them runs, in a private
//! folder in /run (memory too), removed right after.
//!
//! After `unlock`, the unlocked drive is an ordinary drive for UDisks:
//! Library opens (mounts) it as usual.

use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

pub const SOCK: &str = "/run/chlorum-keyd/keyd.sock";
/// Names of the drives this opened: /dev/mapper/chlorum-key-<LUKS UUID>.
const PREFIX: &str = "chlorum-key-";

// ------------------------------------------------------------ client

fn ask(req: Value, wait: Duration) -> Result<(), String> {
    let mut s = UnixStream::connect(SOCK).map_err(|_| "The security-key helper is not running.".to_string())?;
    let _ = s.set_read_timeout(Some(wait));
    s.write_all(format!("{req}\n").as_bytes()).map_err(|e| e.to_string())?;
    let mut line = String::new();
    BufReader::new(s).read_line(&mut line).map_err(|_| "The security key did not answer in time.".to_string())?;
    let v: Value = serde_json::from_str(&line).map_err(|_| "Something went wrong. Please try again.".to_string())?;
    match v.get("error").and_then(|e| e.as_str()) {
        Some(e) => Err(e.to_string()),
        None => Ok(()),
    }
}

/// Opens the encrypted drive at `device` with the security key.
pub async fn unlock(device: String, pin: String) -> Result<(), String> {
    tokio::task::spawn_blocking(move || ask(json!({"op": "unlock", "device": device, "pin": pin}), Duration::from_secs(90)))
        .await
        .map_err(|e| e.to_string())?
}

/// Adds the security key to the drive (its password proves you may).
pub async fn enroll(device: String, password: String, pin: String) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        ask(json!({"op": "enroll", "device": device, "password": password, "pin": pin}), Duration::from_secs(150))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Closes a drive that `unlock` opened (after it was unmounted).
pub async fn lock(device: String) -> Result<(), String> {
    tokio::task::spawn_blocking(move || ask(json!({"op": "lock", "device": device}), Duration::from_secs(30)))
        .await
        .map_err(|e| e.to_string())?
}

// ------------------------------------------------------------ daemon

/// A program asking from inside an app sandbox has its own mount
/// namespace: it may not use the key helper.
fn peer_allowed(stream: &UnixStream) -> bool {
    use std::os::fd::AsRawFd;
    let mut cred = libc::ucred { pid: 0, uid: 0, gid: 0 };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: getsockopt into a correctly sized ucred
    let r = unsafe {
        libc::getsockopt(stream.as_raw_fd(), libc::SOL_SOCKET, libc::SO_PEERCRED, &mut cred as *mut _ as *mut libc::c_void, &mut len)
    };
    if r != 0 || cred.pid <= 0 {
        return false;
    }
    let theirs = std::fs::read_link(format!("/proc/{}/ns/mnt", cred.pid));
    let ours = std::fs::read_link("/proc/self/ns/mnt");
    matches!((theirs, ours), (Ok(a), Ok(b)) if a == b)
}

/// Runs a command with a time limit; returns (success, everything it said).
fn run(cmd: &mut Command, limit: Duration) -> (bool, String) {
    let mut child = match cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn() {
        Ok(c) => c,
        Err(e) => return (false, e.to_string()),
    };
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if start.elapsed() > limit => {
                let _ = child.kill();
                let _ = child.wait();
                return (false, "timed out".into());
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(100)),
            Err(e) => return (false, e.to_string()),
        }
    }
    match child.wait_with_output() {
        Ok(o) => (o.status.success(), format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr))),
        Err(e) => (false, e.to_string()),
    }
}

/// The drive, if it is one this helper may touch: a partition or disk
/// on a USB or removable drive, holding LUKS2. Returns the real device
/// file (links resolved once, so the drive cannot be swapped for another
/// between the checks and the work) and its LUKS UUID.
fn check_device(device: &str) -> Result<(String, String), String> {
    use std::os::unix::fs::FileTypeExt;
    if !device.starts_with("/dev/") {
        return Err("That is not a drive.".into());
    }
    let real = std::fs::canonicalize(device).map_err(|_| "That drive is not there any more.".to_string())?;
    // a block device straight in /dev (devtmpfs: only root makes these)
    let is_block = std::fs::metadata(&real).map(|m| m.file_type().is_block_device()).unwrap_or(false);
    if !is_block || real.parent() != Some(Path::new("/dev")) {
        return Err("That is not a drive.".into());
    }
    let dev = real.to_string_lossy().to_string();
    let lsblk = |path: &str, cols: &str| -> Option<Value> {
        let (ok, out) = run(Command::new("lsblk").args(["-J", "-d", "-o", cols, "--"]).arg(path), Duration::from_secs(10));
        let v: Value = if ok { serde_json::from_str(&out).ok()? } else { return None };
        v.get("blockdevices").and_then(|b| b.get(0)).cloned()
    };
    let info = lsblk(&dev, "FSTYPE,TYPE,PKNAME,RM,TRAN").ok_or("That drive is not there any more.")?;
    let flag = |d: &Value, k: &str| matches!(d.get(k), Some(Value::Bool(true))) || d.get(k).and_then(|x| x.as_str()) == Some("1");
    let text = |d: &Value, k: &str| d.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    // the whole drive it is on: removable media, or on USB (not merely
    // hot-pluggable, which internal eMMC and SATA disks can be too)
    let disk = match text(&info, "pkname") {
        p if !p.is_empty() && text(&info, "type") == "part" => lsblk(&format!("/dev/{p}"), "RM,TRAN,TYPE").unwrap_or_default(),
        _ => info.clone(),
    };
    if !(text(&disk, "type") == "disk" && (flag(&disk, "rm") || text(&disk, "tran") == "usb")) {
        return Err("Only flash drives and other removable drives open this way.".into());
    }
    if text(&info, "fstype") != "crypto_LUKS" {
        return Err("This drive is not encrypted with LUKS, so a security key cannot open it.".into());
    }
    let (ok, _) = run(Command::new("cryptsetup").args(["isLuks", "--type", "luks2", "--"]).arg(&dev), Duration::from_secs(10));
    if !ok {
        return Err("Security keys need a newer kind of encrypted drive (LUKS2).".into());
    }
    let (ok, uuid) = run(Command::new("cryptsetup").args(["luksUUID", "--"]).arg(&dev), Duration::from_secs(10));
    let uuid = uuid.trim().to_string();
    if !ok || uuid.is_empty() || !uuid.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
        return Err("This drive's encryption could not be read.".into());
    }
    Ok((dev, uuid))
}

/// Is a security key plugged in, and does it have a PIN? (None: no key.)
/// Asked first, so that a missing PIN is said at once instead of a
/// password prompt waiting for nobody.
fn key_has_pin() -> Option<bool> {
    let (ok, list) = run(Command::new("fido2-token").arg("-L"), Duration::from_secs(10));
    // "/dev/hidraw3: vendor=0x1050, product=0x0407 (Yubico YubiKey OTP+FIDO+CCID)"
    let path = list.lines().find_map(|l| l.split(':').next().filter(|p| p.starts_with("/dev/hidraw")))?.to_string();
    if !ok {
        return None;
    }
    let (ok, info) = run(Command::new("fido2-token").args(["-I", &path]), Duration::from_secs(10));
    if !ok {
        // it is there, but it says nothing: let the real attempt tell
        return Some(false);
    }
    let options = info.lines().find(|l| l.trim_start().starts_with("options:")).unwrap_or("");
    Some(options.split(',').any(|o| o.trim().trim_start_matches("options:").trim() == "clientPin"))
}

/// The device a drive this helper opened sits on.
fn backing(name: &str) -> Option<String> {
    let (ok, out) = run(Command::new("cryptsetup").args(["status", "--", name]), Duration::from_secs(10));
    if !ok {
        return None;
    }
    out.lines().find_map(|l| l.trim().strip_prefix("device:").map(|d| d.trim().to_string()))
}

/// A private folder for one command's secrets (systemd reads them from
/// $CREDENTIALS_DIRECTORY). Removed when dropped.
struct Secrets(PathBuf);

impl Secrets {
    fn new(files: &[(&str, &str)]) -> Result<Secrets, String> {
        let base = Path::new(SOCK).parent().unwrap().join(format!("secrets-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir(&base).map_err(|e| e.to_string())?;
        std::fs::set_permissions(&base, std::fs::Permissions::from_mode(0o700)).map_err(|e| e.to_string())?;
        for (name, value) in files {
            let p = base.join(name);
            std::fs::write(&p, value).map_err(|e| e.to_string())?;
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o400)).map_err(|e| e.to_string())?;
        }
        Ok(Secrets(base))
    }
}

impl Drop for Secrets {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Plain words for what went wrong (never repeating anything secret).
fn why(out: &str) -> String {
    let o = out.to_lowercase();
    if o.contains("timer expired") || o.contains("too many attempts") || o.contains("operation not permitted") && o.contains("pin") {
        "Wrong PIN, or the key was not touched. Try again.".into()
    } else if o.contains("timed out") {
        "The key was not touched in time. Try again and touch it when it blinks.".into()
    } else if o.contains("pin") && (o.contains("invalid") || o.contains("wrong") || o.contains("incorrect") || o.contains("not accepted")) {
        "Wrong PIN for the key.".into()
    } else if o.contains("pin") && (o.contains("blocked") || o.contains("locked")) {
        "The key is locked after too many wrong PINs. Reset its PIN with the YubiKey tools.".into()
    } else if o.contains("no fido2 device") || o.contains("no device") || o.contains("found no") || o.contains("no security token found") {
        "No security key found. Plug it in and try again.".into()
    } else if o.contains("no fido2") || o.contains("no matching") || o.contains("not suitable") || o.contains("no suitable") || o.contains("no token") {
        "This drive has no security key yet, or this is a different key. Add it first (A).".into()
    } else if o.contains("passphrase") || o.contains("password") || o.contains("no key available") {
        "That password does not open this drive.".into()
    } else {
        "The security key could not be used. Check that it is plugged in, then try again.".into()
    }
}

fn cryptsetup_bin() -> String {
    [std::env::var("CHLORUM_SYSTEMD_CRYPTSETUP").unwrap_or_default(), "/run/current-system/systemd/lib/systemd/systemd-cryptsetup".into()]
        .into_iter()
        .find(|p| !p.is_empty() && Path::new(p).exists())
        // newer systemd also has it among the commands
        .unwrap_or_else(|| "systemd-cryptsetup".into())
}

fn handle(req: &Value) -> Value {
    let s = |k: &str| req.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
    let (device, uuid) = match check_device(&s("device")) {
        Ok(x) => x,
        Err(e) => return json!({"error": e}),
    };
    let name = format!("{PREFIX}{uuid}");
    let mapped = Path::new("/dev/mapper").join(&name);
    match s("op").as_str() {
        "unlock" => {
            if mapped.exists() {
                // the same encryption may be on two drives (one copied from
                // the other): only this drive counts as open
                return match backing(&name) {
                    Some(b) if b == device => json!({"ok": true}),
                    _ => json!({"error": "A copy of this drive is open. Take that one out first."}),
                };
            }
            let pin = s("pin");
            match key_has_pin() {
                None => return json!({"error": "No security key found. Plug it in and try again."}),
                Some(true) if pin.is_empty() => return json!({"error": "This key has a PIN: type it."}),
                _ => {}
            }
            let secrets = match Secrets::new(&[("cryptsetup.fido2-pin", &pin)]) {
                Ok(x) => x,
                Err(e) => return json!({"error": e}),
            };
            let mut c = Command::new(cryptsetup_bin());
            // one try, and no waiting for a typed password if the key does
            // not open it (it would wait for nobody, and say so on every
            // terminal)
            c.args(["attach", &name, &device, "-", "fido2-device=auto,token-timeout=20s,tries=1,timeout=1s"]).env("CREDENTIALS_DIRECTORY", &secrets.0);
            if !pin.is_empty() {
                c.env("PIN", &pin);
            }
            let (ok, out) = run(&mut c, Duration::from_secs(75));
            drop(secrets);
            if ok && mapped.exists() {
                eprintln!("chlorum keyd: opened a drive with a security key");
                json!({"ok": true})
            } else {
                eprintln!("chlorum keyd: could not open a drive with a security key");
                json!({"error": why(&out)})
            }
        }
        "enroll" => {
            let (password, pin) = (s("password"), s("pin"));
            if password.is_empty() {
                return json!({"error": "Type the drive's password first."});
            }
            match key_has_pin() {
                None => return json!({"error": "No security key found. Plug it in and try again."}),
                Some(true) if pin.is_empty() => return json!({"error": "This key has a PIN: type it."}),
                _ => {}
            }
            let secrets = match Secrets::new(&[("cryptenroll.passphrase", &password), ("cryptenroll.fido2-pin", &pin)]) {
                Ok(x) => x,
                Err(e) => return json!({"error": e}),
            };
            let mut c = Command::new("systemd-cryptenroll");
            c.arg("--fido2-device=auto")
                .arg(format!("--fido2-with-client-pin={}", if pin.is_empty() { "no" } else { "yes" }))
                .env("CREDENTIALS_DIRECTORY", &secrets.0)
                .env("PASSWORD", &password)
                .arg("--")
                .arg(&device);
            if !pin.is_empty() {
                c.env("PIN", &pin);
            }
            let (ok, out) = run(&mut c, Duration::from_secs(120));
            drop(secrets);
            if ok {
                eprintln!("chlorum keyd: added a security key to a drive");
                json!({"ok": true})
            } else {
                eprintln!("chlorum keyd: could not add a security key to a drive");
                json!({"error": why(&out)})
            }
        }
        "lock" => {
            if !mapped.exists() {
                return json!({"ok": true});
            }
            if backing(&name).as_deref() != Some(device.as_str()) {
                return json!({"error": "This drive was not opened with a security key."});
            }
            let (ok, out) = run(Command::new(cryptsetup_bin()).args(["detach", &name]), Duration::from_secs(20));
            if ok {
                json!({"ok": true})
            } else if out.to_lowercase().contains("busy") {
                json!({"error": "Something still uses this drive. Close it there first."})
            } else {
                json!({"error": "The drive could not be closed."})
            }
        }
        _ => json!({"error": "Unknown request."}),
    }
}

pub fn run_daemon() -> Result<(), String> {
    let dir = Path::new(SOCK).parent().unwrap();
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(SOCK);
    let listener = UnixListener::bind(SOCK).map_err(|e| e.to_string())?;
    // anyone may connect; who may be served is decided per request
    std::fs::set_permissions(SOCK, std::fs::Permissions::from_mode(0o666)).map_err(|e| e.to_string())?;
    eprintln!("chlorum keyd: ready");
    // Each request is read on its own, so a client that connects and says
    // nothing holds up nobody; the work itself is one at a time (a key can
    // only do one thing at once).
    let work = std::sync::Arc::new(std::sync::Mutex::new(()));
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let work = work.clone();
        std::thread::spawn(move || serve(stream, &work));
    }
    Ok(())
}

fn serve(mut stream: UnixStream, work: &std::sync::Mutex<()>) {
    if !peer_allowed(&stream) {
        let _ = stream.write_all(b"{\"error\":\"Apps may not use security keys this way.\"}\n");
        return;
    }
    let _ = stream.set_read_timeout(Some(Duration::from_secs(3)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(10)));
    let mut line = String::new();
    {
        use std::io::Read;
        let mut limited = (&stream).take(16 * 1024);
        if BufReader::new(&mut limited).read_line(&mut line).is_err() || line.trim().is_empty() {
            return;
        }
    }
    let reply = match serde_json::from_str::<Value>(&line) {
        Ok(req) => {
            let _one = work.lock().unwrap_or_else(|e| e.into_inner());
            handle(&req)
        }
        Err(_) => json!({"error": "Something went wrong. Please try again."}),
    };
    // SAFETY: zeroing the bytes of a String we own keeps it valid UTF-8
    unsafe { line.as_bytes_mut().fill(0) };
    let _ = stream.write_all(format!("{reply}\n").as_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_words() {
        assert_eq!(why("Failed to unlock: PIN is invalid."), "Wrong PIN for the key.");
        assert!(why("timed out").contains("touched"));
        assert!(why("No FIDO2 token found on the volume").contains("no security key yet"));
        assert!(why("No FIDO2 device found.").contains("Plug it in"));
        // a non-drive is refused before anything runs
        assert!(check_device("/etc/passwd").is_err());
        assert!(check_device("/dev/../etc/shadow").is_err());
    }
}
