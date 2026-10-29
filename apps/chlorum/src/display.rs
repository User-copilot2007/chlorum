//! The screen: size of everything (scale), resolution, refresh rate, and
//! the night light. Changed in Center → Screen, applied at once through
//! niri, kept in ~/.config/chlorum/display and applied again at every
//! login (the shell does it when it starts):
//!
//!   scale 1.5
//!   mode 3840x2160@60000        (refresh in millihertz)
//!   night evenings              (off | evenings | always)
//!   warmth 3400                 (kelvin)
//!
//! The night light is wlsunset, run by `chlorum nightlight` (a user
//! service the shell restarts after a change).

use crate::niri;
use niri_ipc::{ConfiguredMode, ModeToSet, OutputAction, Request, Response, ScaleToSet};
use std::path::PathBuf;

/// Chlorum is laid out for about 1920 × 1080 logical pixels: larger sizes
/// would leave too little room on common screens.
pub const SCALES: &[f64] = &[1.0, 1.25, 1.5, 1.75, 2.0, 2.25, 2.5];
pub const WARMTH: &[(u32, &str)] = &[(4500, "Mild"), (3400, "Warm"), (2800, "Warmer")];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Night {
    Off,
    Evenings,
    Always,
}

impl Night {
    pub const ALL: [Night; 3] = [Night::Off, Night::Evenings, Night::Always];
    pub fn key(self) -> &'static str {
        match self {
            Night::Off => "off",
            Night::Evenings => "evenings",
            Night::Always => "always",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Night::Off => "Off",
            Night::Evenings => "Evenings (20:00–7:00)",
            Night::Always => "Always",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub scale: Option<f64>,
    /// (width, height, refresh in mHz)
    pub mode: Option<(u16, u16, u32)>,
    pub night: Night,
    pub warmth: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { scale: None, mode: None, night: Night::Off, warmth: 3400 }
    }
}

fn path() -> PathBuf {
    crate::apps::config_dir().join("chlorum/display")
}

pub fn parse(s: &str) -> Settings {
    let mut out = Settings::default();
    for line in s.lines() {
        let mut w = line.split_whitespace();
        match (w.next(), w.next()) {
            (Some("scale"), Some(v)) => out.scale = v.parse().ok().filter(|x: &f64| (0.5..=4.0).contains(x)),
            (Some("mode"), Some(v)) => out.mode = parse_mode(v),
            (Some("night"), Some(v)) => out.night = Night::ALL.into_iter().find(|n| n.key() == v).unwrap_or(Night::Off),
            (Some("warmth"), Some(v)) => out.warmth = v.parse().ok().filter(|k| (1500..=6500).contains(k)).unwrap_or(3400),
            _ => {}
        }
    }
    out
}

fn parse_mode(v: &str) -> Option<(u16, u16, u32)> {
    let (size, rate) = v.split_once('@')?;
    let (w, h) = size.split_once('x')?;
    Some((w.parse().ok()?, h.parse().ok()?, rate.parse().ok()?))
}

pub fn render(s: &Settings) -> String {
    let mut out = String::new();
    if let Some(x) = s.scale {
        out.push_str(&format!("scale {x}\n"));
    }
    if let Some((w, h, r)) = s.mode {
        out.push_str(&format!("mode {w}x{h}@{r}\n"));
    }
    out.push_str(&format!("night {}\nwarmth {}\n", s.night.key(), s.warmth));
    out
}

pub fn load() -> Settings {
    parse(&std::fs::read_to_string(path()).unwrap_or_default())
}

pub fn save(s: &Settings) -> std::io::Result<()> {
    let p = path();
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d)?;
    }
    let tmp = p.with_extension("new");
    std::fs::write(&tmp, render(s))?;
    std::fs::rename(tmp, p)
}

/// What the screen can do and does now.
#[derive(Clone, Debug, PartialEq)]
pub struct Screen {
    pub name: String,
    pub label: String,
    pub scale: f64,
    /// every mode, (width, height, mHz)
    pub modes: Vec<(u16, u16, u32)>,
    pub current: Option<(u16, u16, u32)>,
}

impl Screen {
    /// Resolutions, largest first.
    pub fn sizes(&self) -> Vec<(u16, u16)> {
        let mut v: Vec<(u16, u16)> = self.modes.iter().map(|m| (m.0, m.1)).collect();
        v.sort_by(|a, b| (b.0 as u32 * b.1 as u32).cmp(&(a.0 as u32 * a.1 as u32)).then(b.0.cmp(&a.0)));
        v.dedup();
        v
    }
    /// Refresh rates at this resolution, fastest first.
    pub fn rates(&self, size: (u16, u16)) -> Vec<u32> {
        let mut v: Vec<u32> = self.modes.iter().filter(|m| (m.0, m.1) == size).map(|m| m.2).collect();
        v.sort_by(|a, b| b.cmp(a));
        v.dedup();
        v
    }
}

/// The screens niri drives (switched on).
pub async fn screens() -> Vec<Screen> {
    let Ok(Response::Outputs(outs)) = niri::send(Request::Outputs).await else { return vec![] };
    let mut v: Vec<Screen> = outs
        .into_values()
        .filter_map(|o| {
            let logical = o.logical?;
            let modes: Vec<(u16, u16, u32)> = o.modes.iter().map(|m| (m.width, m.height, m.refresh_rate)).collect();
            let current = o.current_mode.and_then(|i| modes.get(i).copied());
            let label = format!("{} {}", o.make, o.model).trim().to_string();
            Some(Screen { label: if label.is_empty() { o.name.clone() } else { label }, name: o.name, scale: logical.scale, modes, current })
        })
        .collect();
    v.sort_by(|a, b| a.name.cmp(&b.name));
    v
}

/// The size of everything for a screen nobody chose one for: Chlorum is
/// drawn for about 1080 logical pixels of height, so a 4K screen gets 200%,
/// a 1440p one 125%, a 1080p one 100%.
pub fn auto_scale(height: u16) -> f64 {
    let x = (height as f64 / 1080.0 * 4.0).round() / 4.0;
    x.clamp(1.0, 2.5)
}

/// Puts the settings on every screen (a mode only where the screen has it).
pub async fn apply(s: &Settings) {
    for sc in screens().await {
        let auto = sc.current.map(|(_, h, _)| auto_scale(h));
        if let Some(x) = s.scale.or(auto) {
            if (sc.scale - x).abs() > 0.001 {
                let _ = niri::send(Request::Output { output: sc.name.clone(), action: OutputAction::Scale { scale: ScaleToSet::Specific(x) } }).await;
            }
        }
        if let Some((w, h, r)) = s.mode {
            if sc.modes.contains(&(w, h, r)) && sc.current != Some((w, h, r)) {
                let mode = ConfiguredMode { width: w, height: h, refresh: Some(r as f64 / 1000.0) };
                let _ = niri::send(Request::Output { output: sc.name.clone(), action: OutputAction::Mode { mode: ModeToSet::Specific(mode) } }).await;
            }
        }
    }
}

/// The saved settings put on the screens right away (no async runtime
/// needed): the shell does it before it draws anything, so its first
/// frame is already the right size.
pub fn apply_now() {
    let s = load();
    let Ok(mut sock) = niri_ipc::socket::Socket::connect() else { return };
    let Ok(Ok(Response::Outputs(outs))) = sock.send(Request::Outputs) else { return };
    for o in outs.into_values() {
        let Some(logical) = o.logical else { continue };
        let modes: Vec<(u16, u16, u32)> = o.modes.iter().map(|m| (m.width, m.height, m.refresh_rate)).collect();
        let current = o.current_mode.and_then(|i| modes.get(i).copied());
        let scale = s.scale.or(current.map(|(_, h, _)| auto_scale(h)));
        if let Some(x) = scale {
            if (logical.scale - x).abs() > 0.001 {
                let _ = sock.send(Request::Output { output: o.name.clone(), action: OutputAction::Scale { scale: ScaleToSet::Specific(x) } });
            }
        }
        if let Some((w, h, r)) = s.mode {
            if modes.contains(&(w, h, r)) && current != Some((w, h, r)) {
                let mode = ConfiguredMode { width: w, height: h, refresh: Some(r as f64 / 1000.0) };
                let _ = sock.send(Request::Output { output: o.name.clone(), action: OutputAction::Mode { mode: ModeToSet::Specific(mode) } });
            }
        }
    }
}

/// "3840 × 2160", "60 Hz", "150%".
pub fn size_label(w: u16, h: u16) -> String {
    format!("{w} × {h}")
}
pub fn rate_label(mhz: u32) -> String {
    let hz = mhz as f64 / 1000.0;
    if (hz - hz.round()).abs() < 0.05 { format!("{:.0} Hz", hz) } else { format!("{:.2} Hz", hz) }
}
pub fn scale_label(x: f64) -> String {
    format!("{:.0}%", x * 100.0)
}

/// The wlsunset command line for the settings, or None when it is off.
pub fn night_args(s: &Settings) -> Option<Vec<String>> {
    let low = s.warmth.to_string();
    let (sunset, sunrise) = match s.night {
        Night::Off => return None,
        Night::Evenings => ("20:00", "07:00"),
        // warm all day long: "night" lasts until a minute before it starts again
        Night::Always => ("00:00", "23:59"),
    };
    // half an hour of slow change in the evening and the morning; for
    // "always" only a minute
    let fade = if s.night == Night::Always { "60" } else { "1800" };
    Some(vec!["-t".into(), low, "-T".into(), "6500".into(), "-s".into(), sunset.into(), "-S".into(), sunrise.into(), "-d".into(), fade.into()])
}

/// `chlorum nightlight`: the night light, as the settings say (the user
/// service runs it; it waits when the night light is off).
pub fn nightlight() -> Result<(), String> {
    use std::os::unix::process::CommandExt;
    match night_args(&load()) {
        Some(args) => Err(format!("could not start wlsunset: {}", std::process::Command::new("wlsunset").args(args).exec())),
        None => loop {
            std::thread::park();
        },
    }
}

/// After a change of the night light: the service picks it up.
pub fn restart_nightlight() {
    let _ = std::process::Command::new("systemctl")
        .args(["--user", "restart", "chlorum-nightlight.service"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_round_trip() {
        let s = Settings { scale: Some(1.5), mode: Some((3840, 2160, 59997)), night: Night::Evenings, warmth: 2800 };
        assert_eq!(parse(&render(&s)), s);
        assert_eq!(parse("scale nonsense\nnight sometimes\n"), Settings::default());
        assert_eq!(rate_label(59997), "60 Hz");
        assert_eq!(rate_label(143856), "143.86 Hz");
        assert_eq!(scale_label(1.25), "125%");
        assert!(night_args(&Settings::default()).is_none());
        assert_eq!(night_args(&s).unwrap()[1], "2800");
    }

    #[test]
    fn sizes_and_rates() {
        let sc = Screen {
            name: "DP-1".into(),
            label: "x".into(),
            scale: 1.0,
            modes: vec![(1920, 1080, 60000), (3840, 2160, 60000), (3840, 2160, 30000), (2560, 1440, 59951)],
            current: None,
        };
        assert_eq!(sc.sizes(), vec![(3840, 2160), (2560, 1440), (1920, 1080)]);
        assert_eq!(sc.rates((3840, 2160)), vec![60000, 30000]);
        assert_eq!(auto_scale(2160), 2.0);
        assert_eq!(auto_scale(1440), 1.25);
        assert_eq!(auto_scale(1080), 1.0);
        assert_eq!(auto_scale(768), 1.0);
    }
}
