//! `chlorum setupd` — the first-run helper. Runs as root until the first
//! profile exists, and answers the first-run screen over a socket that only
//! the login screen's group can open. One JSON request per connection, one
//! JSON answer back. Every step is allowed only while setup is not done.

use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

pub const STATE: &str = "/var/lib/chlorum";
pub const SOCK: &str = "/run/chlorum/setup.sock";

/// (xkb layout, console keymap, name)
pub const LAYOUTS: &[(&str, &str, &str)] = &[
    ("us", "us", "English (US)"),
    ("gb", "uk", "English (UK)"),
    ("ua", "ua", "Ukrainian"),
    ("de", "de", "German (QWERTZ)"),
    ("fr", "fr", "French (AZERTY)"),
    ("be", "be-latin1", "Belgian (AZERTY)"),
    ("ch", "de_CH-latin1", "Swiss"),
    ("es", "es", "Spanish"),
    ("it", "it", "Italian"),
    ("pl", "pl", "Polish"),
];

const ADJECTIVES: &[&str] = &[
    "brave", "calm", "clever", "cosmic", "crisp", "curious", "daring", "eager", "fluffy", "frosty", "gentle",
    "giant", "golden", "happy", "hasty", "humble", "jolly", "keen", "lazy", "lively", "lucky", "mellow",
    "mighty", "misty", "nimble", "noble", "odd", "plucky", "polite", "proud", "quick", "quiet", "rapid",
    "rusty", "shiny", "silent", "sleepy", "sly", "snowy", "speedy", "spicy", "steady", "sunny", "swift",
    "tidy", "tiny", "vivid", "wild", "witty", "zesty",
];
const ANIMALS: &[&str] = &[
    "badger", "beaver", "bison", "camel", "cobra", "cougar", "coyote", "crane", "dingo", "falcon", "ferret",
    "gecko", "gopher", "heron", "hippo", "husky", "ibis", "iguana", "jackal", "koala", "lemur", "llama",
    "lynx", "magpie", "marmot", "mole", "moose", "newt", "ocelot", "otter", "owl", "panda", "parrot",
    "pelican", "puffin", "quokka", "rabbit", "raven", "salmon", "seal", "sloth", "tapir", "toucan",
    "turtle", "walrus", "weasel", "wombat", "yak", "zebra", "lobster",
];

fn random(n: usize) -> usize {
    let mut b = [0u8; 8];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        use std::io::Read;
        let _ = f.read_exact(&mut b);
    }
    (u64::from_ne_bytes(b) % n as u64) as usize
}

pub fn new_name() -> String {
    format!("{}-{}", ADJECTIVES[random(ADJECTIVES.len())], ANIMALS[random(ANIMALS.len())])
}

fn read(path: &str) -> String {
    std::fs::read_to_string(path).map(|s| s.trim().to_string()).unwrap_or_default()
}

fn done() -> bool {
    Path::new(&format!("{STATE}/setup-done")).exists()
}

/// Runs a command; (worked, everything it printed).
fn run(cmd: &[&str], env: &[(&str, &str)], timeout: Duration) -> (bool, String) {
    let mut c = Command::new(cmd[0]);
    c.args(&cmd[1..]).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    for (k, v) in env {
        c.env(k, v);
    }
    let Ok(mut child) = c.spawn() else { return (false, format!("{} is missing", cmd[0])) };
    // read what it prints while it runs, so a chatty command never blocks
    let mut so = child.stdout.take().unwrap();
    let mut se = child.stderr.take().unwrap();
    let t1 = std::thread::spawn(move || {
        let mut v = Vec::new();
        let _ = std::io::Read::read_to_end(&mut so, &mut v);
        v
    });
    let t2 = std::thread::spawn(move || {
        let mut v = Vec::new();
        let _ = std::io::Read::read_to_end(&mut se, &mut v);
        v
    });
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if start.elapsed() > timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return (false, "took too long".into());
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(e) => return (false, e.to_string()),
        }
    }
    let status = child.wait();
    let out = t1.join().unwrap_or_default();
    let err = t2.join().unwrap_or_default();
    let text = format!("{}{}", String::from_utf8_lossy(&out), String::from_utf8_lossy(&err));
    match status {
        Ok(s) => (s.success(), text.trim().to_string()),
        Err(e) => (false, e.to_string()),
    }
}

pub fn profiles() -> Vec<String> {
    let (ok, out) = run(&["homectl", "list", "--no-legend"], &[], Duration::from_secs(20));
    if !ok {
        return vec![];
    }
    out.lines().filter_map(|l| l.split_whitespace().next().map(str::to_string)).collect()
}

fn valid_name(n: &str) -> bool {
    let mut c = n.chars();
    matches!(c.next(), Some('a'..='z'))
        && n.len() >= 2
        && n.len() <= 31
        && n.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

struct Helper {
    proposal: Option<(String, String)>,
}

impl Helper {
    fn handle(&mut self, req: &Value) -> Value {
        if done() {
            return json!({"error": "Not allowed."});
        }
        match req["op"].as_str().unwrap_or("") {
            "layouts" => json!({
                "layouts": LAYOUTS.iter().map(|(x, _, n)| json!({"id": x, "name": n})).collect::<Vec<_>>(),
                "current": read(&format!("{STATE}/xkb-layout")),
            }),
            "set_layout" => {
                let id = req["id"].as_str().unwrap_or("");
                match LAYOUTS.iter().find(|l| l.0 == id) {
                    Some((xkb, console, _)) => {
                        let _ = std::fs::write(format!("{STATE}/xkb-layout"), format!("{xkb}\n"));
                        let _ = std::fs::write(format!("{STATE}/keymap"), format!("{console}\n"));
                        run(&["loadkeys", "-q", console], &[], Duration::from_secs(20));
                        json!({"ok": true})
                    }
                    None => json!({"error": "Unknown layout."}),
                }
            }
            "network" => {
                let (ok, out) = run(&["nmcli", "-t", "-f", "CONNECTIVITY", "general"], &[], Duration::from_secs(20));
                if ok && out.trim() == "full" {
                    return json!({"online": true});
                }
                let mut cmd = vec!["nmcli"];
                cmd.extend(crate::system::WIFI_ARGS);
                let (_, out) = run(&cmd, &[], Duration::from_secs(40));
                json!({"online": false, "wifi": crate::system::wifi_scan_blocking(&out)})
            }
            "wifi" => {
                let ssid = req["ssid"].as_str().unwrap_or("");
                let pw = req["password"].as_str().unwrap_or("");
                if ssid.is_empty() {
                    return json!({"error": "Choose a network."});
                }
                let mut cmd = vec!["nmcli", "device", "wifi", "connect", ssid];
                if !pw.is_empty() {
                    cmd.extend(["password", pw]);
                }
                let (ok, _) = run(&cmd, &[], Duration::from_secs(60));
                if ok { json!({"ok": true}) } else { json!({"error": "Could not connect. Check the password and try again."}) }
            }
            "proposal" => {
                if self.proposal.is_none() {
                    let (ok, pw) = run(&["xkcdpass", "-w", "eff-long", "-n", "5", "-d", "-"], &[], Duration::from_secs(20));
                    let pw = pw.lines().next().unwrap_or("").to_string();
                    if !ok || pw.is_empty() {
                        return json!({"error": "Could not make a password."});
                    }
                    let host = read(&format!("{STATE}/hostname"));
                    let mut name = new_name();
                    while name == host {
                        name = new_name();
                    }
                    self.proposal = Some((name, pw));
                }
                let (n, p) = self.proposal.clone().unwrap();
                json!({"name": n, "password": p})
            }
            "create" => {
                let name = req["name"].as_str().unwrap_or("");
                let pw = req["password"].as_str().unwrap_or("");
                if !valid_name(name) {
                    return json!({"error": "Use 2 to 31 lowercase letters, digits or -, starting with a letter."});
                }
                if pw.chars().count() < 12 {
                    return json!({"error": "The password must be at least 12 characters."});
                }
                // A profile left by an earlier attempt was never shown to anyone.
                for old in profiles() {
                    run(&["homectl", "remove", &old], &[], Duration::from_secs(120));
                }
                let real = format!("--real-name={name}");
                let (ok, out) = run(
                    &[
                        "homectl", "create", name, &real, "--member-of=chlorum-owner", "--storage=luks",
                        "--fs-type=btrfs", "--enforce-password-policy=no", "--luks-discard=yes",
                    ],
                    &[("NEWPASSWORD", pw)],
                    Duration::from_secs(600),
                );
                if !ok {
                    eprintln!("chlorum setupd: homectl create failed: {out}");
                    return json!({"error": out.lines().last().unwrap_or("Could not create the profile.")});
                }
                json!({"ok": true})
            }
            "finish" => {
                if profiles().is_empty() {
                    return json!({"error": "No profile was created yet."});
                }
                let _ = std::fs::write(format!("{STATE}/setup-done"), b"");
                std::thread::spawn(|| {
                    std::thread::sleep(Duration::from_secs(1));
                    std::process::exit(0);
                });
                json!({"ok": true})
            }
            _ => json!({"error": "Not allowed."}),
        }
    }
}

pub fn run_daemon() -> Result<(), String> {
    if done() {
        return Ok(());
    }
    let _ = std::fs::create_dir_all(STATE);
    if read(&format!("{STATE}/hostname")).is_empty() {
        let _ = std::fs::write(format!("{STATE}/hostname"), new_name() + "\n");
    }
    let host_file = std::env::var("CHLORUM_HOSTNAME_FILE").unwrap_or_else(|_| "/proc/sys/kernel/hostname".into());
    if let Err(e) = std::fs::write(&host_file, read(&format!("{STATE}/hostname"))) {
        eprintln!("chlorum setupd: hostname: {e}");
    }

    let group = std::env::var("CHLORUM_GREETER_GROUP").unwrap_or_else(|_| "greeter".into());
    let gid: u32 = run(&["getent", "group", &group], &[], Duration::from_secs(10))
        .1
        .split(':')
        .nth(2)
        .and_then(|g| g.trim().parse().ok())
        .ok_or("the login screen's group is missing")?;
    let dir = Path::new(SOCK).parent().unwrap();
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    std::os::unix::fs::chown(dir, Some(0), Some(gid)).map_err(|e| e.to_string())?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o750)).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(SOCK);
    let listener = UnixListener::bind(SOCK).map_err(|e| e.to_string())?;
    std::os::unix::fs::chown(SOCK, Some(0), Some(gid)).map_err(|e| e.to_string())?;
    std::fs::set_permissions(SOCK, std::fs::Permissions::from_mode(0o660)).map_err(|e| e.to_string())?;
    eprintln!("chlorum setupd: ready");

    let mut helper = Helper { proposal: None };
    // one request at a time: creating a profile must never overlap anything
    for stream in listener.incoming() {
        let Ok(mut stream) = stream else { continue };
        // the whole request must arrive within 30 s, however slowly it trickles
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
        let _ = stream.set_write_timeout(Some(Duration::from_secs(10)));
        let mut buf = Vec::new();
        let mut byte = [0u8; 1];
        let mut ok = false;
        while std::time::Instant::now() < deadline && buf.len() < 64 * 1024 {
            use std::io::Read;
            match stream.read(&mut byte) {
                Ok(0) => break,
                Ok(_) if byte[0] == b'\n' => {
                    ok = true;
                    break;
                }
                Ok(_) => buf.push(byte[0]),
                Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => continue,
                Err(_) => break,
            }
        }
        if !ok {
            continue;
        }
        let line = String::from_utf8_lossy(&buf).to_string();
        let reply = match serde_json::from_str::<Value>(&line) {
            Ok(req) => helper.handle(&req),
            Err(_) => json!({"error": "Something went wrong. Please try again."}),
        };
        let _ = stream.write_all(format!("{reply}\n").as_bytes());
    }
    Ok(())
}

/// The login screen's side: one request, one answer.
pub fn call(req: Value) -> Value {
    let Ok(mut s) = UnixStream::connect(SOCK) else {
        return json!({"error": "The setup helper is not running yet. Please wait a moment and try again."});
    };
    let _ = s.set_read_timeout(Some(Duration::from_secs(1200)));
    if s.write_all(format!("{req}\n").as_bytes()).is_err() {
        return json!({"error": "The setup helper did not answer."});
    }
    let mut line = String::new();
    match BufReader::new(&s).read_line(&mut line) {
        Ok(n) if n > 0 => serde_json::from_str(&line).unwrap_or(json!({"error": "The setup helper did not answer."})),
        _ => json!({"error": "The setup helper did not answer."}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn names() {
        assert!(valid_name("brave-otter"));
        assert!(valid_name("ab"));
        assert!(!valid_name("a"));
        assert!(!valid_name("Brave"));
        assert!(!valid_name("1abc"));
        assert!(!valid_name("a b"));
        let n = new_name();
        assert!(valid_name(&n), "{n}");
    }
}
