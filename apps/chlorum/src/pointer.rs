//! `chlorum pointer`: drive the mouse pointer from the keyboard, for the few
//! places that only work with a mouse. Super+M turns it on and off.
//!
//! A bar at the bottom of the screen says which keys do what and keeps the
//! keyboard while the mode is on; it lets clicks fall through to the apps.
//! The pointer itself moves through the compositor's virtual pointer
//! (zwlr_virtual_pointer_v1), like a real mouse would.

use crate::ui::*;
use cosmic::app::{Core, Settings, Task};
use cosmic::iced::platform_specific::runtime::wayland::layer_surface::{IcedOutput, SctkLayerSurfaceSettings};
use cosmic::iced::platform_specific::shell::commands::layer_surface::{Anchor, KeyboardInteractivity, Layer, get_layer_surface};
use cosmic::iced::{self, Alignment, Length, Subscription, event, keyboard, window};
use cosmic::{Application, Element, widget};
use std::collections::HashSet;
use std::sync::mpsc;
use std::time::{Duration, Instant};

// evdev button codes
const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;
const BTN_MIDDLE: u32 = 0x112;

#[derive(Debug)]
enum Do {
    Move(f64, f64),
    Button(u32, bool),
    Scroll(f64),
}

/// The virtual pointer, on its own Wayland connection and thread.
fn pointer_thread() -> Result<mpsc::Sender<Do>, String> {
    use wayland_client::protocol::wl_pointer::{Axis, ButtonState};
    use wayland_client::protocol::wl_registry;
    use wayland_client::{Connection, Dispatch, QueueHandle, globals::GlobalListContents, globals::registry_queue_init};
    use wayland_protocols_wlr::virtual_pointer::v1::client::{zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1, zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1};

    struct S;
    impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for S {
        fn event(_: &mut S, _: &wl_registry::WlRegistry, _: wl_registry::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<S>) {}
    }
    impl Dispatch<ZwlrVirtualPointerManagerV1, ()> for S {
        fn event(_: &mut S, _: &ZwlrVirtualPointerManagerV1, _: <ZwlrVirtualPointerManagerV1 as wayland_client::Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<S>) {}
    }
    impl Dispatch<ZwlrVirtualPointerV1, ()> for S {
        fn event(_: &mut S, _: &ZwlrVirtualPointerV1, _: <ZwlrVirtualPointerV1 as wayland_client::Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<S>) {}
    }

    let conn = Connection::connect_to_env().map_err(|e| e.to_string())?;
    let (globals, mut queue) = registry_queue_init::<S>(&conn).map_err(|e| e.to_string())?;
    let qh = queue.handle();
    let manager: ZwlrVirtualPointerManagerV1 = globals.bind(&qh, 1..=2, ()).map_err(|_| "this compositor has no virtual pointer".to_string())?;
    let ptr = manager.create_virtual_pointer(None, &qh, ());
    let _ = queue.roundtrip(&mut S);
    let (tx, rx) = mpsc::channel::<Do>();
    std::thread::spawn(move || {
        let start = Instant::now();
        let ms = || start.elapsed().as_millis() as u32;
        while let Ok(d) = rx.recv() {
            match d {
                Do::Move(dx, dy) => ptr.motion(ms(), dx, dy),
                Do::Button(b, down) => ptr.button(ms(), b, if down { ButtonState::Pressed } else { ButtonState::Released }),
                // one event: the distance and the wheel clicks it stands for
                Do::Scroll(v) => ptr.axis_discrete(ms(), Axis::VerticalScroll, v, (v / 15.0).round() as i32),
            }
            ptr.frame();
            let _ = conn.flush();
            let _ = queue.dispatch_pending(&mut S);
        }
        ptr.destroy();
        let _ = conn.flush();
    });
    Ok(tx)
}

#[derive(Clone, Debug)]
pub enum Message {
    Frame,
    /// Super+M again, or the pid file gone: time to stop
    Check,
    Mods(keyboard::Modifiers),
    Down(keyboard::Key, keyboard::Modifiers, bool),
    Up(keyboard::Key),
}

pub struct Pointer {
    core: Core,
    tx: Option<mpsc::Sender<Do>>,
    error: Option<String>,
    held: HashSet<&'static str>,
    held_since: Option<Instant>,
    fast: bool,
    fine: bool,
    dragging: bool,
    last: Instant,
}

const BAR: u32 = 64;

impl Application for Pointer {
    type Executor = cosmic::executor::Default;
    type Flags = ();
    type Message = Message;
    const APP_ID: &'static str = "chlorum-pointer";

    fn core(&self) -> &Core {
        &self.core
    }
    fn core_mut(&mut self) -> &mut Core {
        &mut self.core
    }

    fn init(core: Core, _: ()) -> (Self, Task<Message>) {
        let (tx, error) = match pointer_thread() {
            Ok(t) => (Some(t), None),
            Err(e) => (None, Some(e)),
        };
        let p = Pointer { core, tx, error, held: HashSet::new(), held_since: None, fast: false, fine: false, dragging: false, last: Instant::now() };
        let bar = get_layer_surface(SctkLayerSurfaceSettings {
            id: window::Id::unique(),
            layer: Layer::Overlay,
            keyboard_interactivity: KeyboardInteractivity::Exclusive,
            anchor: Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT,
            output: IcedOutput::Active,
            namespace: "chlorum-pointer".into(),
            size: Some((None, Some(BAR))),
            exclusive_zone: 0,
            // clicks go through to the app underneath
            input_zone: Some(vec![]),
            size_limits: iced::Limits::NONE.min_width(1.0).min_height(BAR as f32).max_width(100_000.0).max_height(BAR as f32),
            ..Default::default()
        });
        (p, bar)
    }

    fn subscription(&self) -> Subscription<Message> {
        let mut subs = vec![event::listen_with(|e, _, _| match e {
            iced::Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, repeat, .. }) => Some(Message::Down(key, modifiers, repeat)),
            iced::Event::Keyboard(keyboard::Event::KeyReleased { key, .. }) => Some(Message::Up(key)),
            iced::Event::Keyboard(keyboard::Event::ModifiersChanged(m)) => Some(Message::Mods(m)),
            _ => None,
        })];
        // Super+M again removes the pid file; we notice and stop cleanly
        subs.push(iced::time::every(Duration::from_millis(150)).map(|_| Message::Check));
        if !self.held.is_empty() {
            subs.push(iced::time::every(Duration::from_millis(12)).map(|_| Message::Frame));
        }
        Subscription::batch(subs)
    }

    fn update(&mut self, m: Message) -> Task<Message> {
        match m {
            Message::Frame => self.glide(),
            Message::Mods(m) => {
                self.fast = m.shift();
                self.fine = m.control();
            }
            Message::Check => {
                let mine = std::fs::read_to_string(pid_file()).is_ok_and(|p| p.trim() == std::process::id().to_string());
                if !mine {
                    return self.quit();
                }
            }
            Message::Down(key, mods, repeat) => {
                self.fast = mods.shift();
                self.fine = mods.control();
                if let Some(dir) = direction(&key) {
                    if self.held.is_empty() {
                        self.held_since = Some(Instant::now());
                        self.last = Instant::now();
                    }
                    if self.held.insert(dir) {
                        // a tap moves a little even before the glide starts
                        let (dx, dy) = step(dir, if self.fine { 1.0 } else { 6.0 });
                        self.send(Do::Move(dx, dy));
                    }
                    return Task::none();
                }
                if repeat {
                    return Task::none();
                }
                use keyboard::key::Named;
                match key.as_ref() {
                    keyboard::Key::Named(Named::Escape) => return self.quit(),
                    keyboard::Key::Named(Named::Enter) if mods.shift() => self.click(BTN_RIGHT),
                    keyboard::Key::Named(Named::Enter) => self.click(BTN_LEFT),
                    keyboard::Key::Named(Named::PageUp) => self.send(Do::Scroll(-45.0)),
                    keyboard::Key::Named(Named::PageDown) => self.send(Do::Scroll(45.0)),
                    keyboard::Key::Character(c) => match c.to_lowercase().as_str() {
                        " " => self.click(BTN_LEFT),
                        "r" => self.click(BTN_RIGHT),
                        "c" => self.click(BTN_MIDDLE),
                        "d" => {
                            self.dragging = !self.dragging;
                            self.send(Do::Button(BTN_LEFT, self.dragging));
                        }
                        "u" => self.send(Do::Scroll(-45.0)),
                        "n" => self.send(Do::Scroll(45.0)),
                        "q" => return self.quit(),
                        _ => {}
                    },
                    _ => {}
                }
            }
            Message::Up(key) => {
                if let Some(dir) = direction(&key) {
                    self.held.remove(dir);
                    if self.held.is_empty() {
                        self.held_since = None;
                    }
                }
            }
        }
        Task::none()
    }

    fn view(&self) -> Element<'_, Message> {
        widget::Space::new().into()
    }

    fn view_window(&self, _id: window::Id) -> Element<'_, Message> {
        let mut row = widget::Row::new()
            .push(fill(text("MOUSE", MONO_BOLD, 16.0, BLACK), YELLOW).padding([6, 14]))
            .push(space(22.0, 1.0))
            .spacing(0)
            .align_y(Alignment::Center);
        if let Some(e) = &self.error {
            row = row.push(text(format!("Not available: {e}"), SANS, 18.0, RED));
        } else {
            let keys: &[(&str, &str, bool)] = &[
                ("← ↑ ↓ →", "Move", false),
                ("SHIFT", "Fast", false),
                ("CTRL", "Fine", false),
                ("ENTER", "Click", true),
                ("R", "Right click", false),
                ("D", if self.dragging { "Drop" } else { "Drag" }, self.dragging),
                ("PGUP PGDN", "Scroll", false),
                ("ESC", "Done", false),
            ];
            row = row.push(hints(keys, 1.0));
        }
        fill(row.padding([0, 28]).height(Length::Fill), Color::from_rgba(0.035, 0.035, 0.035, 0.94)).width(Length::Fill).height(Length::Fill).into()
    }
}

use cosmic::iced::Color;

impl Pointer {
    fn send(&self, d: Do) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(d);
        }
    }

    fn click(&self, b: u32) {
        self.send(Do::Button(b, true));
        self.send(Do::Button(b, false));
    }

    /// While arrows are held the pointer glides, faster the longer they are.
    fn glide(&mut self) {
        let dt = self.last.elapsed().as_secs_f64().min(0.05);
        self.last = Instant::now();
        let held_for = self.held_since.map(|t| t.elapsed().as_secs_f64()).unwrap_or(0.0);
        if held_for < 0.18 {
            return;
        }
        // pixels per second: from 250 up to 1600, x3 with Shift, slow with Ctrl
        let mut speed = (250.0 + 900.0 * (held_for - 0.18)).min(1600.0);
        if self.fast {
            speed *= 3.0;
        }
        if self.fine {
            speed = 60.0;
        }
        let (mut dx, mut dy) = (0.0, 0.0);
        for d in &self.held {
            let (x, y) = step(d, speed * dt);
            dx += x;
            dy += y;
        }
        if dx != 0.0 || dy != 0.0 {
            self.send(Do::Move(dx, dy));
        }
    }

    fn quit(&mut self) -> Task<Message> {
        if self.dragging {
            self.send(Do::Button(BTN_LEFT, false));
        }
        self.tx = None;
        // let the pointer thread send its last events
        std::thread::sleep(Duration::from_millis(30));
        let pf = pid_file();
        if std::fs::read_to_string(&pf).is_ok_and(|p| p.trim() == std::process::id().to_string()) {
            let _ = std::fs::remove_file(pf);
        }
        cosmic::iced::exit()
    }
}

fn direction(key: &keyboard::Key) -> Option<&'static str> {
    use keyboard::key::Named;
    match key.as_ref() {
        keyboard::Key::Named(Named::ArrowLeft) => Some("left"),
        keyboard::Key::Named(Named::ArrowRight) => Some("right"),
        keyboard::Key::Named(Named::ArrowUp) => Some("up"),
        keyboard::Key::Named(Named::ArrowDown) => Some("down"),
        keyboard::Key::Character(c) => match c.to_lowercase().as_str() {
            "h" => Some("left"),
            "l" => Some("right"),
            "k" => Some("up"),
            "j" => Some("down"),
            _ => None,
        },
        _ => None,
    }
}

fn step(dir: &str, d: f64) -> (f64, f64) {
    match dir {
        "left" => (-d, 0.0),
        "right" => (d, 0.0),
        "up" => (0.0, -d),
        _ => (0.0, d),
    }
}

fn pid_file() -> std::path::PathBuf {
    crate::apps::runtime_dir().join("pointer.pid")
}

/// Super+M: turn the mode on, or off when it is already on.
pub fn run() -> Result<(), String> {
    let pf = pid_file();
    if let Ok(pid) = std::fs::read_to_string(&pf) {
        let pid = pid.trim().to_string();
        let running = std::fs::read(format!("/proc/{pid}/cmdline")).is_ok_and(|c| {
            let args: Vec<&[u8]> = c.split(|b| *b == 0).collect();
            args.len() >= 2 && args[0].ends_with(b"chlorum") && args[1] == b"pointer"
        });
        let _ = std::fs::remove_file(&pf);
        if running {
            // it sees the file gone, lets go of any drag and stops; if it
            // hangs, stop it the hard way
            std::thread::sleep(Duration::from_millis(800));
            if std::fs::metadata(format!("/proc/{pid}")).is_ok()
                && std::fs::read(format!("/proc/{pid}/cmdline")).is_ok_and(|c| c.split(|b| *b == 0).nth(1) == Some(b"pointer"))
            {
                let _ = std::process::Command::new("kill").arg(&pid).status();
            }
            return Ok(());
        }
    }
    let _ = std::fs::create_dir_all(crate::apps::runtime_dir());
    let _ = std::fs::write(&pf, std::process::id().to_string());
    // a small bar: drawn on the CPU, no graphics card needed
    if std::env::var_os("ICED_BACKEND").is_none() {
        // SAFETY: nothing else runs yet
        unsafe { std::env::set_var("ICED_BACKEND", "tiny-skia") };
    }
    let settings = Settings::default().no_main_window(true).default_font(SANS).transparent(true);
    let r = cosmic::app::run::<Pointer>(settings, ()).map_err(|e| e.to_string());
    let _ = std::fs::remove_file(&pf);
    r
}

// ==================================================================== numpad

/// The numpad is the mouse, always: niri sends each numpad key here
/// (`chlorum mouse go left`, `chlorum mouse click` …), and this service moves
/// the pointer. Holding a key glides it, faster the longer it is held.
///
///   8 4 6 2      move          7 9 1 3   move on the diagonal
///   5            click         /         right click
///   *            middle click  0         hold the button (drag), again to drop
///   −  +         scroll up / down
fn mouse_socket() -> std::path::PathBuf {
    crate::apps::runtime_dir().join("mouse.sock")
}

/// How the glide speeds up: pixels per second after holding for `t` s.
fn glide_speed(t: f64) -> f64 {
    (220.0 + 1100.0 * t).min(2200.0)
}

/// Parts of a direction word: "upleft" → up and left.
fn dirs(word: &str) -> Vec<&'static str> {
    let mut v = Vec::new();
    for d in ["up", "down", "left", "right"] {
        if word.contains(d) {
            v.push(d);
        }
    }
    v
}

struct Held {
    /// direction → (pressed since, key still held until)
    keys: std::collections::HashMap<&'static str, (Instant, Instant)>,
}

/// `chlorum mouse` runs the service; `chlorum mouse <words…>` tells it.
pub fn mouse_cli(args: &[String]) -> Result<(), String> {
    use std::io::Write;
    if !args.is_empty() {
        let mut s = std::os::unix::net::UnixStream::connect(mouse_socket()).map_err(|e| e.to_string())?;
        return s.write_all(format!("{}\n", args.join(" ")).as_bytes()).map_err(|e| e.to_string());
    }
    let tx = pointer_thread()?;
    // a drag cut short by a restart must not leave the button held
    let _ = tx.send(Do::Button(BTN_LEFT, false));
    let held = std::sync::Arc::new(std::sync::Mutex::new(Held { keys: Default::default() }));
    let dragging = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

    // the glide: while a key repeats, the pointer moves every few ms
    {
        let held = held.clone();
        let tx = tx.clone();
        std::thread::spawn(move || {
            let mut last = Instant::now();
            loop {
                std::thread::sleep(Duration::from_millis(8));
                let dt = last.elapsed().as_secs_f64().min(0.05);
                last = Instant::now();
                let now = Instant::now();
                let (mut dx, mut dy) = (0.0, 0.0);
                if let Ok(mut h) = held.lock() {
                    h.keys.retain(|_, (_, until)| *until > now);
                    for (d, (since, _)) in &h.keys {
                        // a tap already moved a little; the glide starts
                        // when the key starts repeating (300 ms)
                        let t = since.elapsed().as_secs_f64();
                        if t < 0.3 {
                            continue;
                        }
                        let (x, y) = step(d, glide_speed(t - 0.3) * dt);
                        dx += x;
                        dy += y;
                    }
                }
                if dx != 0.0 || dy != 0.0 {
                    let _ = tx.send(Do::Move(dx, dy));
                }
            }
        });
    }

    let path = mouse_socket();
    let _ = std::fs::create_dir_all(crate::apps::runtime_dir());
    let _ = std::fs::remove_file(&path);
    let listener = std::os::unix::net::UnixListener::bind(&path).map_err(|e| e.to_string())?;
    eprintln!("chlorum: numpad mouse ready");
    for stream in listener.incoming().flatten() {
        let mut line = String::new();
        let _ = std::io::BufRead::read_line(&mut std::io::BufReader::new(stream), &mut line);
        let words: Vec<&str> = line.split_whitespace().collect();
        let click = |b: u32| {
            let _ = tx.send(Do::Button(b, true));
            let _ = tx.send(Do::Button(b, false));
        };
        match words.as_slice() {
            ["go", word] => {
                let now = Instant::now();
                if let Ok(mut h) = held.lock() {
                    for d in dirs(word) {
                        // the first repeat comes 300 ms after the press, then
                        // one every 25 ms: wait that long before deciding the
                        // key was let go
                        let entry = h.keys.entry(d).or_insert((now, now));
                        let fresh = entry.1 <= now;
                        if fresh {
                            *entry = (now, now);
                            let (x, y) = step(d, 8.0);
                            let _ = tx.send(Do::Move(x, y));
                        }
                        let first = now.duration_since(entry.0) < Duration::from_millis(250);
                        entry.1 = now + Duration::from_millis(if first { 380 } else { 140 });
                    }
                }
            }
            ["click"] => click(BTN_LEFT),
            ["right"] => click(BTN_RIGHT),
            ["middle"] => click(BTN_MIDDLE),
            ["drag"] => {
                let down = !dragging.load(std::sync::atomic::Ordering::Relaxed);
                dragging.store(down, std::sync::atomic::Ordering::Relaxed);
                let _ = tx.send(Do::Button(BTN_LEFT, down));
            }
            ["scroll", "up"] => {
                let _ = tx.send(Do::Scroll(-45.0));
            }
            ["scroll", "down"] => {
                let _ = tx.send(Do::Scroll(45.0));
            }
            _ => eprintln!("chlorum: mouse: unknown {line:?}"),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn numpad_words() {
        assert_eq!(super::dirs("upleft"), vec!["up", "left"]);
        assert_eq!(super::dirs("down"), vec!["down"]);
        assert!(super::glide_speed(0.0) < super::glide_speed(1.0));
        assert_eq!(super::glide_speed(100.0), 2200.0);
    }

    #[test]
    fn steps() {
        assert_eq!(super::step("left", 5.0), (-5.0, 0.0));
        assert_eq!(super::step("down", 5.0), (0.0, 5.0));
    }
}
