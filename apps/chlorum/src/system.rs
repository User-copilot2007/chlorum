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

/// A speaker, headphones, a monitor's sound, a microphone…
#[derive(Clone, Debug, PartialEq)]
pub struct AudioDev {
    /// PipeWire's id (for wpctl)
    pub id: u32,
    pub name: String,
    pub default: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Devices {
    pub outputs: Vec<AudioDev>,
    pub inputs: Vec<AudioDev>,
    pub mic_volume: f32,
    pub mic_muted: bool,
    pub mic_present: bool,
}

/// Every sound output and input PipeWire knows, which ones are used, and
/// the microphone's level.
pub async fn devices() -> Devices {
    let mut d = match output("pw-dump", &[]).await {
        Ok(s) => parse_devices(&s),
        Err(_) => Devices::default(),
    };
    if let Ok(s) = output("wpctl", &["get-volume", "@DEFAULT_AUDIO_SOURCE@"]).await {
        d.mic_volume = s.split_whitespace().nth(1).and_then(|v| v.parse().ok()).unwrap_or(0.0);
        d.mic_muted = s.contains("MUTED");
        d.mic_present = !d.inputs.is_empty();
    }
    d
}

fn parse_devices(json: &str) -> Devices {
    let v: serde_json::Value = serde_json::from_str(json).unwrap_or_default();
    let Some(objs) = v.as_array() else { return Devices::default() };
    // the names of the ones in use
    let mut default_sink = String::new();
    let mut default_source = String::new();
    for o in objs {
        if o.get("type").and_then(|t| t.as_str()) != Some("PipeWire:Interface:Metadata") {
            continue;
        }
        if o.pointer("/props/metadata.name").and_then(|n| n.as_str()) != Some("default") {
            continue;
        }
        for m in o.get("metadata").and_then(|m| m.as_array()).into_iter().flatten() {
            let name = m.pointer("/value/name").and_then(|n| n.as_str()).unwrap_or("").to_string();
            match m.get("key").and_then(|k| k.as_str()) {
                Some("default.audio.sink") => default_sink = name,
                Some("default.audio.source") => default_source = name,
                _ => {}
            }
        }
    }
    let mut d = Devices::default();
    for o in objs {
        if o.get("type").and_then(|t| t.as_str()) != Some("PipeWire:Interface:Node") {
            continue;
        }
        let Some(props) = o.pointer("/info/props") else { continue };
        let class = props.get("media.class").and_then(|c| c.as_str()).unwrap_or("");
        let id = o.get("id").and_then(|i| i.as_u64()).unwrap_or(0) as u32;
        let node = props.get("node.name").and_then(|c| c.as_str()).unwrap_or("");
        let name = props
            .get("node.description")
            .or_else(|| props.get("node.nick"))
            .and_then(|c| c.as_str())
            .unwrap_or(node)
            .to_string();
        match class {
            "Audio/Sink" => d.outputs.push(AudioDev { id, name, default: node == default_sink }),
            "Audio/Source" => d.inputs.push(AudioDev { id, name, default: node == default_source }),
            _ => {}
        }
    }
    d.outputs.sort_by(|a, b| b.default.cmp(&a.default).then(a.name.cmp(&b.name)));
    d.inputs.sort_by(|a, b| b.default.cmp(&a.default).then(a.name.cmp(&b.name)));
    d
}

pub async fn set_default_device(id: u32) -> Devices {
    let _ = output("wpctl", &["set-default", &id.to_string()]).await;
    devices().await
}

pub async fn set_mic_volume(v: f32) -> Devices {
    let v = v.clamp(0.0, 1.0);
    let _ = output("wpctl", &["set-volume", "@DEFAULT_AUDIO_SOURCE@", &format!("{v:.2}")]).await;
    let _ = output("wpctl", &["set-mute", "@DEFAULT_AUDIO_SOURCE@", "0"]).await;
    devices().await
}

pub async fn toggle_mic() -> Devices {
    let _ = output("wpctl", &["set-mute", "@DEFAULT_AUDIO_SOURCE@", "toggle"]).await;
    devices().await
}

/// A short chime on the output in use: is sound coming out, and where?
pub async fn test_sound() -> Result<(), String> {
    let path = std::env::temp_dir().join("chlorum-test-sound.wav");
    std::fs::write(&path, chime()).map_err(|e| e.to_string())?;
    output("pw-play", &[path.to_string_lossy().as_ref()]).await.map(|_| ())
}

/// Two soft notes (A5, E6), 16-bit mono WAV.
fn chime() -> Vec<u8> {
    const RATE: u32 = 48000;
    let mut samples: Vec<i16> = vec![];
    for (freq, secs) in [(880.0f32, 0.35f32), (1318.5, 0.5)] {
        let n = (RATE as f32 * secs) as usize;
        for i in 0..n {
            let t = i as f32 / RATE as f32;
            // quick rise, slow fall
            let env = (t * 60.0).min(1.0) * (-t * 6.0).exp();
            samples.push((f32::sin(2.0 * std::f32::consts::PI * freq * t) * env * 0.35 * i16::MAX as f32) as i16);
        }
    }
    let data_len = (samples.len() * 2) as u32;
    let mut w = Vec::with_capacity(44 + data_len as usize);
    w.extend(b"RIFF");
    w.extend((36 + data_len).to_le_bytes());
    w.extend(b"WAVEfmt ");
    w.extend(16u32.to_le_bytes());
    w.extend(1u16.to_le_bytes());
    w.extend(1u16.to_le_bytes());
    w.extend(RATE.to_le_bytes());
    w.extend((RATE * 2).to_le_bytes());
    w.extend(2u16.to_le_bytes());
    w.extend(16u16.to_le_bytes());
    w.extend(b"data");
    w.extend(data_len.to_le_bytes());
    for s in samples {
        w.extend(s.to_le_bytes());
    }
    w
}

#[cfg(test)]
mod sound_tests {
    #[test]
    fn devices_from_pw_dump() {
        let j = r#"[
          {"id":40,"type":"PipeWire:Interface:Node","info":{"props":{"media.class":"Audio/Sink","node.name":"alsa_output.hdmi","node.description":"AD104 HDMI / DisplayPort 1 Output"}}},
          {"id":41,"type":"PipeWire:Interface:Node","info":{"props":{"media.class":"Audio/Sink","node.name":"alsa_output.analog","node.description":"Built-in Audio Analog Stereo"}}},
          {"id":42,"type":"PipeWire:Interface:Node","info":{"props":{"media.class":"Audio/Source","node.name":"alsa_input.analog","node.description":"Built-in Audio Analog Stereo"}}},
          {"id":43,"type":"PipeWire:Interface:Node","info":{"props":{"media.class":"Stream/Output/Audio","node.name":"firefox"}}},
          {"id":30,"type":"PipeWire:Interface:Metadata","props":{"metadata.name":"default"},"metadata":[
            {"subject":0,"key":"default.audio.sink","type":"Spa:String:JSON","value":{"name":"alsa_output.hdmi"}},
            {"subject":0,"key":"default.audio.source","type":"Spa:String:JSON","value":{"name":"alsa_input.analog"}}]}
        ]"#;
        let d = super::parse_devices(j);
        assert_eq!(d.outputs.len(), 2);
        assert!(d.outputs[0].default && d.outputs[0].id == 40);
        assert_eq!(d.inputs.len(), 1);
        assert!(d.inputs[0].default);
        assert_eq!(&super::chime()[..4], b"RIFF");
    }
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
    /// the version installed now
    UpToDate(String),
    /// the version that will be installed
    Available(String),
    /// how far it is (None until the first numbers come)
    Installing(Option<crate::progress::Progress>),
    /// the version that starts after the next restart
    Installed(String),
    Failed(String),
}

impl Update {
    pub fn text(&self) -> String {
        match self {
            Update::Idle => "Not checked yet".into(),
            Update::Checking => "Checking…".into(),
            Update::UpToDate(v) if v.is_empty() => "Chlorum is up to date".into(),
            Update::UpToDate(v) => format!("Chlorum is up to date ({v})"),
            Update::Available(v) if v.is_empty() => "A new version is ready to install".into(),
            Update::Available(v) => format!("Chlorum {v} is ready to install"),
            Update::Installing(None) => "Starting the update…".into(),
            Update::Installing(Some(p)) => {
                let v = if p.version.is_empty() { String::new() } else { format!(" {}", p.version) };
                match p.phase.as_str() {
                    "checking" => "Looking for the new version…".into(),
                    "building" => format!("Preparing Chlorum{v} · {}%", p.percent()),
                    "installing" => format!("Installing Chlorum{v}…"),
                    _ => format!("Downloading Chlorum{v} · {}%", p.percent()),
                }
            }
            Update::Installed(v) if v.is_empty() => "Installed. It starts after the next restart.".into(),
            Update::Installed(v) => format!("Chlorum {v} is installed. It starts after the next restart."),
            Update::Failed(e) => format!("Did not work: {e}"),
        }
    }

    /// 0..1 for the line, while installing
    pub fn fraction(&self) -> Option<f32> {
        match self {
            Update::Installing(Some(p)) if p.phase == "installing" => Some(1.0),
            Update::Installing(Some(p)) => Some(p.percent() as f32 / 100.0),
            Update::Installing(None) => Some(0.0),
            _ => None,
        }
    }
}

pub async fn check_update() -> Update {
    match output("chlorum-update", &["check"]).await {
        Ok(s) => {
            let s = s.trim();
            let (state, version) = s.split_once(' ').unwrap_or((s, ""));
            match state {
                "available" => Update::Available(version.to_string()),
                "up-to-date" => Update::UpToDate(version.to_string()),
                _ => Update::Failed(if s.is_empty() { "no answer".into() } else { s.into() }),
            }
        }
        Err(e) => Update::Failed(e),
    }
}

/// Starts the install in the background; the progress file tells the rest.
pub async fn install_update() -> Update {
    match output("chlorum-update", &["apply"]).await {
        Ok(_) => Update::Installing(None),
        Err(e) => Update::Failed(e),
    }
}

/// What an update (started here or by the daily timer) is doing.
pub fn update_progress() -> Option<crate::progress::Progress> {
    crate::progress::read(&std::path::PathBuf::from("/run/chlorum-update/update.json"))
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

// ---------------------------------------------------------------- Wi-Fi

#[derive(Clone, Debug, PartialEq)]
pub struct Wifi {
    pub ssid: String,
    pub signal: u8,
    pub secure: bool,
    pub active: bool,
}

/// nmcli -t escapes ':' inside values as '\:'.
fn split_terse(line: &str) -> Vec<String> {
    let mut out = vec![String::new()];
    let mut esc = false;
    for c in line.chars() {
        if esc {
            out.last_mut().unwrap().push(c);
            esc = false;
        } else if c == '\\' {
            esc = true;
        } else if c == ':' {
            out.push(String::new());
        } else {
            out.last_mut().unwrap().push(c);
        }
    }
    out
}

pub fn parse_wifi(text: &str) -> Vec<Wifi> {
    let mut v: Vec<Wifi> = Vec::new();
    for line in text.lines() {
        let f = split_terse(line);
        if f.len() < 4 || f[1].is_empty() {
            continue;
        }
        let w = Wifi {
            active: f[0] == "*",
            ssid: f[1].clone(),
            signal: f[2].parse().unwrap_or(0),
            secure: !(f[3].is_empty() || f[3] == "--"),
        };
        match v.iter_mut().find(|x| x.ssid == w.ssid) {
            Some(x) => {
                x.active |= w.active;
                x.signal = x.signal.max(w.signal);
            }
            None => v.push(w),
        }
    }
    v.sort_by(|a, b| b.active.cmp(&a.active).then(b.signal.cmp(&a.signal)));
    v.truncate(12);
    v
}

pub const WIFI_ARGS: &[&str] = &["-t", "-f", "IN-USE,SSID,SIGNAL,SECURITY", "device", "wifi", "list", "--rescan", "yes"];

/// For the first-run helper (no async runtime there).
pub fn wifi_scan_blocking(text: &str) -> serde_json::Value {
    serde_json::Value::Array(
        parse_wifi(text)
            .into_iter()
            .map(|w| serde_json::json!({"ssid": w.ssid, "signal": w.signal, "secure": w.secure}))
            .collect(),
    )
}

pub async fn wifi_list() -> Vec<Wifi> {
    match output("nmcli", WIFI_ARGS).await {
        Ok(s) => parse_wifi(&s),
        Err(_) => vec![],
    }
}

/// Is the Wi-Fi radio on? (None: there is no Wi-Fi here)
pub async fn wifi_on() -> Option<bool> {
    let s = output("nmcli", &["-t", "-f", "WIFI-HW,WIFI", "radio"]).await.ok()?;
    let line = s.lines().next()?;
    let (hw, on) = line.split_once(':')?;
    if hw.trim() == "missing" {
        return None;
    }
    Some(on.trim() == "enabled")
}

pub async fn set_wifi(on: bool) -> Result<(), String> {
    output("nmcli", &["radio", "wifi", if on { "on" } else { "off" }]).await.map(|_| ()).map_err(|e| {
        let why = e.lines().last().unwrap_or("").trim().trim_start_matches("Error:").trim().to_string();
        if why.is_empty() { "Could not switch Wi-Fi.".to_string() } else { why }
    })
}

/// Leaves the Wi-Fi network it is on (it is not forgotten).
pub async fn wifi_disconnect(ssid: String) -> Result<(), String> {
    output("nmcli", &["connection", "down", "id", ssid.as_str()]).await.map(|_| ()).map_err(|e| {
        let why = e.lines().last().unwrap_or("").trim().trim_start_matches("Error:").trim().to_string();
        if why.is_empty() { "Could not disconnect.".to_string() } else { why }
    })
}

pub async fn wifi_connect(ssid: String, password: String) -> Result<(), String> {
    let mut args = vec!["device", "wifi", "connect", ssid.as_str()];
    if !password.is_empty() {
        args.extend(["password", password.as_str()]);
    }
    output("nmcli", &args).await.map(|_| ()).map_err(|e| {
        // nmcli says why: wrong password, network gone, not allowed…
        let why = e.lines().last().unwrap_or("").trim().trim_start_matches("Error:").trim().to_string();
        if why.is_empty() { "Could not connect.".to_string() } else { format!("Could not connect: {why}") }
    })
}

// ---------------------------------------------------------------- keyboard

/// The person's own layout, or the one chosen on first start.
pub fn keyboard() -> String {
    std::fs::read_to_string(config_dir().join("chlorum/keyboard"))
        .or_else(|_| std::fs::read_to_string("/var/lib/chlorum/xkb-layout"))
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "us".into())
}

/// The niri layout string: the chosen layout plus US. Passwords are Latin
/// words, so a non-Latin layout (Ukrainian) comes second: what you type at
/// the login and lock screens is always Latin, Alt+Shift switches.
pub fn xkb_layouts(id: &str) -> String {
    // US QWERTY always comes first: passwords are made and typed on it, so
    // the login and lock screens read them the same way. The other layout is
    // one Alt+Shift away.
    match id {
        "us" | "" => "us".into(),
        other => format!("us,{other}"),
    }
}

/// Saves the layout and switches to it now: niri reloads its config file.
pub fn set_keyboard(id: &str) {
    let dir = config_dir().join("chlorum");
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::fs::write(dir.join("keyboard"), format!("{id}\n"));
    if let Ok(path) = std::env::var("CHLORUM_NIRI_CONFIG") {
        set_niri_layout(std::path::Path::new(&path), id);
    }
}

pub fn set_niri_layout(path: &std::path::Path, id: &str) {
    let Ok(text) = std::fs::read_to_string(path) else { return };
    let new: Vec<String> = text
        .lines()
        .map(|l| {
            if l.trim_start().starts_with("layout \"") {
                let indent = &l[..l.len() - l.trim_start().len()];
                format!("{indent}layout \"{}\"", xkb_layouts(id))
            } else {
                l.to_string()
            }
        })
        .collect();
    let tmp = path.with_extension("new");
    if std::fs::write(&tmp, new.join("\n") + "\n").is_ok() {
        let _ = std::fs::rename(tmp, path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wifi_lines() {
        let v = parse_wifi("*:Home\\:5G:70:WPA2\n :Cafe:80:\n :Home\\:5G:40:WPA2\n ::30:WPA2\n");
        assert_eq!(v.len(), 2);
        assert_eq!(v[0], Wifi { ssid: "Home:5G".into(), signal: 70, secure: true, active: true });
        assert_eq!(v[1], Wifi { ssid: "Cafe".into(), signal: 80, secure: false, active: false });
    }
    #[test]
    fn layouts() {
        assert_eq!(xkb_layouts("ua"), "us,ua");
        assert_eq!(xkb_layouts("de"), "us,de");
        assert_eq!(xkb_layouts("us"), "us");
    }
}
