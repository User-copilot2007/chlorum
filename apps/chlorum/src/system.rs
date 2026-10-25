//! Small pieces of the system the shell shows and changes: wallpaper
//! settings, sound, network, updates, power.

use crate::apps::{config_dir, runtime_dir};
use std::process::Stdio;

// ---------------------------------------------------------------- wallpaper

pub const STYLES: &[(&str, &str)] = &[
    ("lava", "Lava lamp"),
    ("gas", "Chlorine gas"),
    ("beams", "Light beams"),
    ("particles", "Particles"),
    ("shells", "Electron shells"),
    ("none", "Plain black"),
];

pub const COLORS: &[(&str, &str)] = &[
    ("#FFD60A", "Chlorum yellow"),
    ("#C8E640", "Chlorine green"),
    ("#4FD1FF", "Ice blue"),
    ("#FF6B4A", "Ember"),
    ("#F5F5F5", "White"),
];

#[derive(Clone, Debug, PartialEq)]
pub struct Wallpaper {
    pub style: String,
    pub speed: f32,
    pub brightness: f32,
    pub amount: f32, // 1..=10
    pub color: String,
}

impl Default for Wallpaper {
    fn default() -> Self {
        Wallpaper { style: "lava".into(), speed: 0.4, brightness: 0.7, amount: 6.0, color: "#FFD60A".into() }
    }
}

impl Wallpaper {
    pub fn load() -> Wallpaper {
        let mut w = Wallpaper::default();
        if let Ok(text) = std::fs::read_to_string(config_dir().join("chlorum/wallpaper")) {
            for line in text.lines() {
                let Some((k, v)) = line.split_once('=') else { continue };
                let v = v.trim();
                match k.trim() {
                    "style" => w.style = v.into(),
                    "speed" => w.speed = v.parse().unwrap_or(w.speed),
                    "brightness" => w.brightness = v.parse().unwrap_or(w.brightness),
                    "amount" => w.amount = v.parse().unwrap_or(w.amount),
                    "color" => w.color = v.into(),
                    _ => {}
                }
            }
        }
        w
    }

    pub fn save(&self) {
        let dir = config_dir().join("chlorum");
        let _ = std::fs::create_dir_all(&dir);
        let text = format!(
            "style={}\nspeed={:.2}\nbrightness={:.2}\namount={}\ncolor={}\n",
            self.style, self.speed, self.brightness, self.amount.round(), self.color
        );
        let tmp = dir.join(".wallpaper.new");
        if std::fs::write(&tmp, text).is_ok() {
            let _ = std::fs::rename(tmp, dir.join("wallpaper"));
        }
    }
}

/// The wallpaper stops drawing while an app covers the whole screen.
pub fn pause_wallpaper(paused: bool) {
    let dir = runtime_dir();
    let flag = dir.join("wallpaper-paused");
    if paused {
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(flag, b"");
    } else {
        let _ = std::fs::remove_file(flag);
    }
}

// ---------------------------------------------------------------- commands

async fn output(cmd: &str, args: &[&str]) -> Result<String, String> {
    let out = tokio::process::Command::new(cmd)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .await
        .map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    if out.status.success() {
        Ok(text)
    } else {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        Err(if err.is_empty() { text.trim().to_string() } else { err })
    }
}

// ---------------------------------------------------------------- sound

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Sound {
    pub volume: f32,
    pub muted: bool,
    pub present: bool,
}

pub async fn sound() -> Sound {
    match output("wpctl", &["get-volume", "@DEFAULT_AUDIO_SINK@"]).await {
        // "Volume: 0.40 [MUTED]"
        Ok(s) => {
            let volume = s.split_whitespace().nth(1).and_then(|v| v.parse().ok()).unwrap_or(0.0);
            Sound { volume, muted: s.contains("MUTED"), present: true }
        }
        Err(_) => Sound::default(),
    }
}

pub async fn set_volume(v: f32) -> Sound {
    let v = v.clamp(0.0, 1.0);
    let _ = output("wpctl", &["set-volume", "@DEFAULT_AUDIO_SINK@", &format!("{v:.2}")]).await;
    let _ = output("wpctl", &["set-mute", "@DEFAULT_AUDIO_SINK@", "0"]).await;
    sound().await
}

pub async fn toggle_mute() -> Sound {
    let _ = output("wpctl", &["set-mute", "@DEFAULT_AUDIO_SINK@", "toggle"]).await;
    sound().await
}

// ---------------------------------------------------------------- network

pub async fn network() -> String {
    let Ok(s) = output("nmcli", &["-t", "-f", "TYPE,STATE,CONNECTION", "device"]).await else {
        return "Offline".into();
    };
    for line in s.lines() {
        let f: Vec<&str> = line.splitn(3, ':').collect();
        if f.len() == 3 && f[1] == "connected" {
            return match f[0] {
                "ethernet" => "Cable".into(),
                "wifi" => f[2].replace("\\:", ":"),
                other => other.to_string(),
            };
        }
    }
    "Offline".into()
}

// ---------------------------------------------------------------- updates

#[derive(Clone, Debug, PartialEq)]
pub enum Update {
    Idle,
    Checking,
    UpToDate,
    Available,
    Installing,
    Installed,
    Failed(String),
}

impl Update {
    pub fn text(&self) -> String {
        match self {
            Update::Idle => "Not checked yet".into(),
            Update::Checking => "Checking…".into(),
            Update::UpToDate => "Chlorum is up to date".into(),
            Update::Available => "A new version is ready to install".into(),
            Update::Installing => "Installing in the background. Keep working.".into(),
            Update::Installed => "Installed. It starts after the next restart.".into(),
            Update::Failed(e) => format!("Did not work: {e}"),
        }
    }
}

pub async fn check_update() -> Update {
    match output("chlorum-update", &["check"]).await {
        Ok(s) if s.trim() == "available" => Update::Available,
        Ok(s) if s.trim() == "up-to-date" => Update::UpToDate,
        Ok(s) => Update::Failed(if s.trim().is_empty() { "no answer".into() } else { s.trim().into() }),
        Err(e) => Update::Failed(e),
    }
}

pub async fn install_update() -> Update {
    match output("chlorum-update", &["apply"]).await {
        Ok(_) => Update::Installed,
        Err(e) => Update::Failed(e),
    }
}

// ---------------------------------------------------------------- power

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Power {
    LogOut,
    Restart,
    ShutDown,
}

impl Power {
    pub const ALL: [Power; 3] = [Power::LogOut, Power::Restart, Power::ShutDown];
    pub fn label(self) -> &'static str {
        match self {
            Power::LogOut => "Log out",
            Power::Restart => "Restart",
            Power::ShutDown => "Shut down",
        }
    }
    pub fn run(self) {
        let (cmd, args): (&str, &[&str]) = match self {
            Power::LogOut => ("niri", &["msg", "action", "quit", "--skip-confirmation"]),
            Power::Restart => ("systemctl", &["reboot"]),
            Power::ShutDown => ("systemctl", &["poweroff"]),
        };
        let _ = std::process::Command::new(cmd).args(args).stdin(Stdio::null()).spawn();
    }
}

pub fn version() -> String {
    std::fs::read_to_string("/etc/chlorum-release")
        .ok()
        .and_then(|s| {
            s.lines()
                .find_map(|l| l.strip_prefix("VERSION=").map(|v| v.trim_matches('"').to_string()))
                .or_else(|| s.lines().next().map(|l| l.trim().trim_start_matches("Chlorum ").to_string()))
        })
        .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string())
}

pub fn computer_name() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname").map(|s| s.trim().to_string()).unwrap_or_default()
}

/// The person's display name from their profile, or the login name.
pub fn display_name() -> String {
    let user = std::env::var("USER").unwrap_or_default();
    let out = std::process::Command::new("getent").args(["passwd", &user]).output();
    if let Ok(o) = out {
        let line = String::from_utf8_lossy(&o.stdout).to_string();
        if let Some(gecos) = line.split(':').nth(4) {
            let name = gecos.split(',').next().unwrap_or("").trim();
            if !name.is_empty() {
                return name.to_string();
            }
        }
    }
    user
}
