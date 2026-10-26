//! `chlorum idle`: when nobody has touched the computer for a while, the
//! screen turns off, and a little later the screen locks. Apps that play a
//! video or a call keep it awake (they ask niri, and niri does not report
//! the computer as idle while they do).
//!
//! The times live in ~/.config/chlorum/idle and are changed in Center →
//! Screen:
//!   screen-off 300
//!   lock 600
//! (seconds; 0 means never). The service starts over when the file changes.

use std::path::PathBuf;
use std::time::{Duration, SystemTime};

/// Choices offered in Center, in seconds (0 = never).
pub const SCREEN_OFF: &[u32] = &[0, 60, 120, 300, 600, 900, 1800];
pub const LOCK: &[u32] = &[0, 60, 300, 600, 900, 1800, 3600];

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Times {
    pub screen_off: u32,
    pub lock: u32,
}

impl Default for Times {
    fn default() -> Self {
        Times { screen_off: 300, lock: 600 }
    }
}

fn path() -> PathBuf {
    crate::apps::config_dir().join("chlorum/idle")
}

pub fn load() -> Times {
    parse(&std::fs::read_to_string(path()).unwrap_or_default())
}

fn parse(s: &str) -> Times {
    let mut t = Times::default();
    for line in s.lines() {
        let mut it = line.split_whitespace();
        let (Some(k), Some(v)) = (it.next(), it.next().and_then(|v| v.parse::<u32>().ok())) else { continue };
        match k {
            "screen-off" => t.screen_off = v,
            "lock" => t.lock = v,
            _ => {}
        }
    }
    t
}

pub fn save(t: Times) -> std::io::Result<()> {
    let p = path();
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d)?;
    }
    let tmp = p.with_extension("new");
    std::fs::write(&tmp, format!("screen-off {}\nlock {}\n", t.screen_off, t.lock))?;
    std::fs::rename(tmp, p)
}

/// "5 minutes", "1 hour", "Never".
pub fn label(secs: u32) -> String {
    match secs {
        0 => "Never".into(),
        s if s % 3600 == 0 => {
            let h = s / 3600;
            format!("{h} hour{}", if h == 1 { "" } else { "s" })
        }
        s => {
            let m = (s / 60).max(1);
            format!("{m} minute{}", if m == 1 { "" } else { "s" })
        }
    }
}

/// The next choice in a list, `dir` steps on (−1 or +1).
pub fn next(list: &[u32], now: u32, dir: i32) -> u32 {
    let i = list.iter().position(|v| *v == now).unwrap_or(0) as i32;
    list[(i + dir).clamp(0, list.len() as i32 - 1) as usize]
}

fn act(what: &str) {
    use crate::niri::{Action, Request, send};
    let action = match what {
        "off" => Action::PowerOffMonitors {},
        _ => Action::SpawnSh { command: "chlorum lock".into() },
    };
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build();
    if let Ok(rt) = rt {
        if let Err(e) = rt.block_on(send(Request::Action(action))) {
            eprintln!("chlorum: idle: {e}");
        }
    }
}

pub fn run() -> Result<(), String> {
    use wayland_client::protocol::{wl_registry, wl_seat::WlSeat};
    use wayland_client::{Connection, Dispatch, QueueHandle, globals::GlobalListContents, globals::registry_queue_init};
    use wayland_protocols::ext::idle_notify::v1::client::{
        ext_idle_notification_v1::{self, ExtIdleNotificationV1},
        ext_idle_notifier_v1::ExtIdleNotifierV1,
    };

    struct S;
    impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for S {
        fn event(_: &mut S, _: &wl_registry::WlRegistry, _: wl_registry::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<S>) {}
    }
    impl Dispatch<WlSeat, ()> for S {
        fn event(_: &mut S, _: &WlSeat, _: <WlSeat as wayland_client::Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<S>) {}
    }
    impl Dispatch<ExtIdleNotifierV1, ()> for S {
        fn event(_: &mut S, _: &ExtIdleNotifierV1, _: <ExtIdleNotifierV1 as wayland_client::Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<S>) {}
    }
    impl Dispatch<ExtIdleNotificationV1, &'static str> for S {
        fn event(_: &mut S, _: &ExtIdleNotificationV1, e: ext_idle_notification_v1::Event, what: &&'static str, _: &Connection, _: &QueueHandle<S>) {
            if let ext_idle_notification_v1::Event::Idled = e {
                eprintln!("chlorum: idle: {what}");
                act(what);
            }
        }
    }

    let times = load();
    // start over when Center changes the times
    let watched = path();
    let stamp = |p: &PathBuf| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    let first: Option<SystemTime> = stamp(&watched);
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(Duration::from_secs(2));
            if stamp(&watched) != first {
                eprintln!("chlorum: idle: times changed, starting over");
                // systemd starts it again (RestartForceExitStatus)
                std::process::exit(75);
            }
        }
    });

    let conn = Connection::connect_to_env().map_err(|e| e.to_string())?;
    let (globals, mut queue) = registry_queue_init::<S>(&conn).map_err(|e| e.to_string())?;
    let qh = queue.handle();
    let seat: WlSeat = globals.bind(&qh, 1..=1, ()).map_err(|_| "no seat".to_string())?;
    let notifier: ExtIdleNotifierV1 = globals.bind(&qh, 1..=1, ()).map_err(|_| "this compositor cannot tell when the computer is idle".to_string())?;
    let mut keep = Vec::new();
    if times.screen_off > 0 {
        keep.push(notifier.get_idle_notification(times.screen_off.saturating_mul(1000), &seat, &qh, "off"));
    }
    if times.lock > 0 {
        keep.push(notifier.get_idle_notification(times.lock.saturating_mul(1000), &seat, &qh, "lock"));
    }
    eprintln!("chlorum: idle: screen off {}, lock {}", label(times.screen_off), label(times.lock));
    loop {
        queue.blocking_dispatch(&mut S).map_err(|e| e.to_string())?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times() {
        assert_eq!(parse(""), Times::default());
        assert_eq!(parse("screen-off 60\nlock 0\n"), Times { screen_off: 60, lock: 0 });
        assert_eq!(label(0), "Never");
        assert_eq!(label(60), "1 minute");
        assert_eq!(label(300), "5 minutes");
        assert_eq!(label(3600), "1 hour");
        assert_eq!(next(SCREEN_OFF, 300, 1), 600);
        assert_eq!(next(SCREEN_OFF, 0, -1), 0);
        assert_eq!(next(LOCK, 3600, 1), 3600);
    }
}
