//! Chlorum Shell: everything the system itself shows.
//!
//! * Home: clock, the ribbon of apps, Center and All apps. Shown when no app
//!   is open, or when chosen from the system menu.
//! * System menu: the PS-button menu, opened with Super over any app.
//! * The templates: one app on the whole screen, or two side by side.
//!
//! Every screen is driven with the arrows, Enter and Esc. Hints at the bottom
//! say which keys do what.

use crate::apps::{self, App};
use crate::layout::{Layout, Stage};
use crate::niri::{self, Action, Event, WorkspaceReferenceArg};
use crate::notify::{self, Note};
use crate::system::{self, Power, Sound, Update, Wallpaper, Wifi};
use crate::ui::*;
use chrono::{Local, Offset, Timelike};
use cosmic::app::{Core, Settings, Task};
use cosmic::iced::futures::SinkExt;
use cosmic::iced::platform_specific::runtime::wayland::layer_surface::{IcedMargin, IcedOutput, SctkLayerSurfaceSettings};
use cosmic::widget::text_input;
use cosmic::iced::platform_specific::shell::commands::layer_surface::{
    Anchor, KeyboardInteractivity, Layer, get_layer_surface, set_keyboard_interactivity, set_layer, set_size,
};
use cosmic::iced::{self, Alignment, Color, Length, Subscription, event, keyboard, window};
use cosmic::{Application, Element, widget};
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, BufReader};

/// What the shell does, for `journalctl --user -u chlorum-shell`.
macro_rules! say {
    ($($t:tt)*) => { eprintln!("chlorum: {}", format!($($t)*)) };
}

/// When the shell last handled anything (ms since START).
static BEAT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();

fn beat() {
    let ms = START.get_or_init(Instant::now).elapsed().as_millis() as u64;
    BEAT.store(ms, std::sync::atomic::Ordering::Relaxed);
}

/// The shell redraws its clock four times a second. If nothing has been
/// handled for 8 seconds, it is stuck (a driver wait, a bug): leave, and
/// systemd starts a fresh one a second later, with the apps still open.
fn watchdog() {
    START.get_or_init(Instant::now);
    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_secs(20));
        loop {
            std::thread::sleep(Duration::from_secs(2));
            let now = START.get().unwrap().elapsed().as_millis() as u64;
            let last = BEAT.load(std::sync::atomic::Ordering::Relaxed);
            if last > 0 && now.saturating_sub(last) > 8000 {
                eprintln!("chlorum: shell stopped responding for {} s; restarting", (now - last) / 1000);
                std::process::exit(3);
            }
        }
    });
}

pub fn run() -> cosmic::iced::Result {
    watchdog();
    let settings = Settings::default()
        .no_main_window(true)
        .default_font(SANS)
        .transparent(true)
        .default_icon_theme("Papirus-Dark");
    cosmic::app::run::<Shell>(settings, ())
}

/// `chlorum msg <what>`: what niri's key bindings send to the running shell.
pub fn msg(what: &str) -> std::io::Result<()> {
    use std::io::Write;
    let mut s = std::os::unix::net::UnixStream::connect(apps::runtime_dir().join("shell.sock"))?;
    s.write_all(format!("{what}\n").as_bytes())
}

const SUPER_DELAY: Duration = Duration::from_millis(140);

#[derive(Clone, Debug)]
enum Tile {
    Running { id: u64, name: String, icon: String },
    App(App),
    Center,
    AllApps,
}

impl Tile {
    fn key(&self) -> String {
        match self {
            Tile::Running { id, .. } => format!("w{id}"),
            Tile::App(a) => format!("a{}", a.id),
            Tile::Center => "center".into(),
            Tile::AllApps => "all".into(),
        }
    }
    fn name(&self) -> &str {
        match self {
            Tile::Running { name, .. } => name,
            Tile::App(a) => &a.name,
            Tile::Center => "Center",
            Tile::AllApps => "All apps",
        }
    }
    fn icon(&self) -> &str {
        match self {
            Tile::Running { icon, .. } => icon,
            Tile::App(a) => &a.icon,
            Tile::Center => "preferences-system",
            Tile::AllApps => "view-app-grid-symbolic",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum HomeAct {
    Open,
    Beside,
    Close,
}

impl HomeAct {
    fn label(self) -> &'static str {
        match self {
            HomeAct::Open => "Open",
            HomeAct::Beside => "Open beside",
            HomeAct::Close => "Close app",
        }
    }
    fn key(self) -> &'static str {
        match self {
            HomeAct::Open => "ENTER",
            HomeAct::Beside => "S",
            HomeAct::Close => "X",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Page {
    Main,
    AllApps,
    Center,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Section {
    Wallpaper,
    Network,
    Sound,
    Keyboard,
    Screen,
    Apps,
    Updates,
    About,
}

impl Section {
    const ALL: [Section; 8] =
        [Section::Wallpaper, Section::Network, Section::Sound, Section::Keyboard, Section::Screen, Section::Apps, Section::Updates, Section::About];
    fn label(self) -> &'static str {
        match self {
            Section::Wallpaper => "Wallpaper",
            Section::Network => "Network",
            Section::Sound => "Sound",
            Section::Keyboard => "Keyboard",
            Section::Screen => "Screen",
            Section::Apps => "Apps",
            Section::Updates => "Updates",
            Section::About => "About",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Card {
    Layout,
    Apps,
    Notes,
    Update,
    Sound,
    Home,
    Center,
    Power,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Sub {
    None,
    Apps(usize),
    Power(usize),
    Sound,
    Notes(usize),
}

#[derive(Clone, Debug)]
pub enum Message {
    Frame,
    Tick,
    Key(window::Id, keyboard::Key, keyboard::Modifiers),
    Niri(Event),
    Arranged,
    StartupOver,
    Command(String),
    SuperFire(u64),
    Nudge(window::Id, Option<u32>, Option<u32>),
    Sound(Sound),
    Network(String),
    Update(Update),
    WifiList(Vec<Wifi>),
    WifiDone(Result<(), String>),
    WifiInput(String),
    WifiSubmit,
    Note(notify::Event),
}

pub struct Shell {
    core: Core,
    layout: Layout,
    installed: HashMap<String, App>,
    /// Center → Apps: what each app may use, and the app being looked at
    denied: crate::sandbox::Denied,
    perm_app: Option<String>,
    /// Center → Screen: when the screen turns off and locks by itself
    idle: crate::idle::Times,
    /// notifications wait in the menu without popping up
    dnd: bool,
    pinned: Vec<String>,
    started: bool,

    /// The one full-screen surface Home and the menu are drawn on. It is made
    /// once and never destroyed: on NVIDIA, making and destroying
    /// full-screen surfaces is what froze the shell. Hidden, it waits below
    /// the apps, transparent and without the keyboard.
    surface: window::Id,
    surface_up: bool,
    /// The small surface notifications appear on, made once too.
    note_id: window::Id,

    /// Home is showing (on `surface`)
    home: Option<window::Id>,
    home_show: f32,
    home_closing: bool,
    page: Page,
    sel: usize,
    act: Option<usize>,
    grow: HashMap<String, f32>,
    all_sel: usize,
    section: usize,
    row: Option<usize>,

    /// The menu is showing (on `surface`)
    menu: Option<window::Id>,
    menu_show: f32,
    menu_closing: bool,
    card: usize,
    sub: Sub,
    power_armed: bool,

    wallpaper: Wallpaper,
    sound: Sound,
    network: String,
    update: Update,
    user: String,

    super_seq: u64,
    /// focus moves the shell asked for itself; niri reports them back
    expect_focus: Vec<u64>,
    quiet_until: Instant,
    arranging: bool,
    arrange_again: bool,
    /// Home closed: show the apps' workspace again
    back_to_stage: bool,
    toast: Option<(String, Instant)>,
    last_frame: Instant,

    wifi: Vec<Wifi>,
    wifi_busy: bool,
    wifi_entry: Option<String>,
    wifi_pw: String,
    wifi_note: Option<String>,
    keyboard: String,

    notes: Vec<Note>,
    note_surface: Option<(window::Id, Instant)>,

    started_at: Instant,
    ids: [widget::Id; 1], // Wi-Fi password
}

impl Application for Shell {
    type Executor = cosmic::executor::Default;
    type Flags = ();
    type Message = Message;
    const APP_ID: &'static str = "com.chlorum.Shell";

    fn core(&self) -> &Core {
        &self.core
    }
    fn core_mut(&mut self) -> &mut Core {
        &mut self.core
    }

    fn init(core: Core, _: ()) -> (Self, Task<Message>) {
        let mut shell = Shell {
            core,
            layout: Layout::default(),
            installed: apps::installed(),
            denied: crate::sandbox::load(),
            perm_app: None,
            idle: crate::idle::load(),
            dnd: dnd_path().exists(),
            pinned: apps::id_list("home-apps", apps::DEFAULT_PINNED),
            started: false,
            home: None,
            surface: window::Id::unique(),
            surface_up: false,
            note_id: window::Id::unique(),
            home_show: 0.0,
            home_closing: false,
            page: Page::Main,
            sel: 0,
            act: None,
            grow: HashMap::new(),
            all_sel: 0,
            section: 0,
            row: None,
            menu: None,
            menu_show: 0.0,
            menu_closing: false,
            card: 0,
            sub: Sub::None,
            power_armed: false,
            wallpaper: Wallpaper::load(),
            sound: Sound::default(),
            network: String::new(),
            update: Update::Idle,
            user: system::display_name(),
            super_seq: 0,
            expect_focus: Vec::new(),
            quiet_until: Instant::now(),
            arranging: false,
            arrange_again: false,
            back_to_stage: false,
            toast: None,
            last_frame: Instant::now(),
            wifi: vec![],
            wifi_busy: false,
            wifi_entry: None,
            wifi_pw: String::new(),
            wifi_note: None,
            keyboard: system::keyboard(),
            notes: vec![],
            note_surface: None,
            started_at: Instant::now(),
            ids: [widget::Id::unique()],
        };
        system::pause_wallpaper(false);
        let made = Task::batch([
            open_surface(layer(shell.surface, Layer::Background, KeyboardInteractivity::None, "chlorum-shell")),
            open_surface(note_layer(shell.note_id)),
        ]);
        let t = Task::batch([
            made,
            shell.open_home(),
            Task::perform(system::sound(), |s| cosmic::Action::App(Message::Sound(s))),
            Task::perform(system::network(), |s| cosmic::Action::App(Message::Network(s))),
        ]);
        (shell, t)
    }

    fn subscription(&self) -> Subscription<Message> {
        let mut subs = vec![
            Subscription::run(niri::events).map(Message::Niri),
            Subscription::run(commands),
            Subscription::run(notify::events).map(Message::Note),
            Subscription::run(notify::sleeps).map(|_| Message::Command("lock".into())),
            Subscription::run(crate::drives::plugged).map(|name| {
                Message::Note(notify::Event::Show(Note {
                    id: 0x4472_0000 | (name.len() as u32 & 0xffff),
                    app: "Drives".into(),
                    icon: "drive-removable-media".into(),
                    summary: format!("{name} is plugged in"),
                    body: "Open it in Library, under Drives.".into(),
                }))
            }),
            iced::time::every(Duration::from_millis(250)).map(|_| Message::Tick),
            event::listen_with(|e, _status, id| match e {
                iced::Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. }) => {
                    Some(Message::Key(id, key, modifiers))
                }
                _ => None,
            }),
        ];
        if self.animating() || self.wifi_busy {
            subs.push(iced::time::every(Duration::from_millis(8)).map(|_| Message::Frame));
        }
        Subscription::batch(subs)
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        beat();
        let t = self.handle(message);
        self.sync_grow();
        if std::mem::take(&mut self.back_to_stage) {
            if let Some(f) = self.layout.focused() {
                self.expect_focus.push(f);
                return Task::batch([t, Task::perform(niri::run(vec![Action::FocusWindow { id: f }]), |_| cosmic::Action::App(Message::Tick))]);
            }
        }
        t
    }

    fn view(&self) -> Element<'_, Message> {
        widget::Space::new().into()
    }

    fn view_window(&self, id: window::Id) -> Element<'_, Message> {
        if id == self.note_id {
            return if self.note_surface.is_some() { self.note_view() } else { widget::Space::new().into() };
        }
        if id != self.surface {
            return widget::Space::new().into();
        }
        match (self.home.is_some(), self.menu.is_some()) {
            (true, true) => cosmic::iced::widget::Stack::new()
                .push(self.home_view())
                .push(self.menu_view())
                .width(Length::Fill)
                .height(Length::Fill)
                .into(),
            (true, false) => self.home_view(),
            (false, true) => self.menu_view(),
            (false, false) => widget::Space::new().into(),
        }
    }
}

impl Shell {
    fn handle(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Frame => return self.step(),
            Message::Tick => {
                // frames late or missing: finish the move anyway, never leave
                // the menu or Home half-open
                if self.animating() && self.last_frame.elapsed() > Duration::from_millis(200) {
                    self.last_frame = Instant::now() - Duration::from_secs(1);
                    let t = self.step();
                    let t2 = self.step();
                    return Task::batch([t, t2]);
                }
                if self.toast.as_ref().is_some_and(|(_, t)| t.elapsed() > Duration::from_secs(3)) {
                    self.toast = None;
                }
                if let Some((_, at)) = self.note_surface {
                    if at.elapsed() > Duration::from_secs(5) {
                        self.note_surface = None;
                        return set_layer(self.note_id, Layer::Background);
                    }
                }
                // sound and network change outside the shell too: look every few seconds
                let now = Local::now().second();
                if now % 5 == 0 && Local::now().timestamp_subsec_millis() < 250 {
                    return Task::batch([
                        Task::perform(system::sound(), |s| cosmic::Action::App(Message::Sound(s))),
                        Task::perform(system::network(), |s| cosmic::Action::App(Message::Network(s))),
                    ]);
                }
            }
            Message::Sound(s) => self.sound = s,
            Message::Network(n) => self.network = n,
            Message::Update(u) => {
                let start = u == Update::Available && self.update == Update::Installing;
                self.update = u;
                if start {
                    return Task::perform(system::install_update(), |u| cosmic::Action::App(Message::Update(u)));
                }
            }
            Message::Niri(ev) => return self.niri_event(ev),
            Message::StartupOver => self.layout.startup_left = 0,
            Message::Arranged => {
                self.arranging = false;
                self.quiet_until = Instant::now() + Duration::from_millis(300);
                if std::mem::take(&mut self.arrange_again) {
                    return self.arrange();
                }
            }
            Message::Command(c) => return self.command(c.trim()),
            Message::Nudge(id, w, h) => {
                return set_size(id, w, h);
            }
            Message::SuperFire(seq) => {
                if seq != self.super_seq {
                    say!("super {seq} cancelled by a later key ({})", self.super_seq);
                }
                if seq == self.super_seq {
                    return self.toggle_menu();
                }
            }
            Message::WifiList(v) => {
                self.wifi = v;
                self.wifi_busy = false;
            }
            Message::WifiDone(r) => {
                self.wifi_busy = false;
                match r {
                    Ok(()) => {
                        self.wifi_entry = None;
                        self.wifi_note = Some("Connected.".into());
                        return Task::batch([
                            Task::perform(system::network(), |s| cosmic::Action::App(Message::Network(s))),
                            Task::perform(system::wifi_list(), |v| cosmic::Action::App(Message::WifiList(v))),
                        ]);
                    }
                    Err(e) => {
                        self.wifi_note = Some(e);
                        self.wifi_pw.clear();
                        if self.wifi_entry.is_some() {
                            return text_input::focus(self.ids[0].clone());
                        }
                    }
                }
            }
            Message::WifiInput(v) => self.wifi_pw = v,
            Message::WifiSubmit => {
                if let Some(ssid) = self.wifi_entry.clone() {
                    self.wifi_busy = true;
                    self.wifi_note = Some(format!("Connecting to {ssid}…"));
                    return Task::perform(system::wifi_connect(ssid, self.wifi_pw.clone()), |r| {
                        cosmic::Action::App(Message::WifiDone(r))
                    });
                }
            }
            Message::Note(notify::Event::Show(n)) => return self.show_note(n),
            Message::Note(notify::Event::Close(id)) => self.notes.retain(|n| n.id != id),
            Message::Key(id, key, mods) => {
                if Some(id) == self.menu && !self.menu_closing {
                    return self.menu_key(key);
                }
                if Some(id) == self.home && !self.home_closing {
                    return self.home_key(key, mods);
                }
            }
        }
        Task::none()
    }

    /// Every tile has a size that eases between resting and chosen.
    fn sync_grow(&mut self) {
        let keys: Vec<String> = self.tiles().iter().map(|t| t.key()).collect();
        self.grow.retain(|k, _| keys.contains(k));
        for k in keys {
            if !self.grow.contains_key(&k) {
                let g = self.grow_target(&k);
                self.grow.insert(k, g);
            }
        }
    }
}

/// Lines written to $XDG_RUNTIME_DIR/chlorum/shell.sock by `chlorum msg`.
fn commands() -> impl cosmic::iced::futures::Stream<Item = Message> {
    cosmic::iced::stream::channel(16, async |mut out| {
        let dir = apps::runtime_dir();
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("shell.sock");
        let _ = std::fs::remove_file(&path);
        let Ok(listener) = tokio::net::UnixListener::bind(&path) else { return };
        loop {
            let Ok((stream, _)) = listener.accept().await else { continue };
            let mut line = String::new();
            if BufReader::new(stream).read_line(&mut line).await.is_ok() && !line.trim().is_empty() {
                if out.send(Message::Command(line)).await.is_err() {
                    return;
                }
            }
        }
    })
}

/// The notification surface: bottom right, never takes the keyboard. Made
/// hidden (below the apps).
fn note_layer(id: window::Id) -> SctkLayerSurfaceSettings {
    SctkLayerSurfaceSettings {
        id,
        layer: Layer::Background,
        keyboard_interactivity: KeyboardInteractivity::None,
        anchor: Anchor::BOTTOM | Anchor::RIGHT,
        output: IcedOutput::Active,
        namespace: "chlorum-note".into(),
        margin: IcedMargin { top: 0, right: 28, bottom: 96, left: 0 },
        size: Some((Some(480), Some(132))),
        exclusive_zone: 0,
        size_limits: iced::Limits::NONE.min_width(1.0).min_height(1.0).max_width(480.0).max_height(132.0),
        ..Default::default()
    }
}

/// A new surface, plus two nudges: re-sending the size makes the toolkit
/// configure and draw it again, in case its first redraw was lost. On a
/// healthy surface it changes nothing.
fn open_surface(settings: SctkLayerSurfaceSettings) -> Task<Message> {
    let id = settings.id;
    let (w, h) = settings.size.unwrap_or((None, None));
    let nudge = |ms| {
        Task::perform(tokio::time::sleep(Duration::from_millis(ms)), move |_| cosmic::Action::App(Message::Nudge(id, w, h)))
    };
    Task::batch([get_layer_surface(settings), nudge(400), nudge(1200)])
}

fn dnd_path() -> std::path::PathBuf {
    apps::config_dir().join("chlorum/do-not-disturb")
}

fn layer(id: window::Id, layer: Layer, keyboard: KeyboardInteractivity, namespace: &str) -> SctkLayerSurfaceSettings {
    SctkLayerSurfaceSettings {
        id,
        layer,
        keyboard_interactivity: keyboard,
        anchor: Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT,
        output: IcedOutput::Active,
        namespace: namespace.into(),
        size: Some((None, None)),
        exclusive_zone: -1,
        size_limits: iced::Limits::NONE.min_width(1.0).min_height(1.0).max_width(100_000.0).max_height(100_000.0),
        ..Default::default()
    }
}

impl Shell {
    // ================================================================ surfaces

    fn open_home(&mut self) -> Task<Message> {
        self.home_closing = false;
        self.page = Page::Main;
        self.act = None;
        self.sel = 0;
        self.kick();
        system::pause_wallpaper(false);
        // slide the apps away: Home has its own empty workspace, so the live
        // wallpaper shows behind it
        let away = if self.layout.on_stage().is_empty() {
            Task::none()
        } else {
            Task::perform(
                niri::run(vec![Action::FocusWorkspace { reference: WorkspaceReferenceArg::Name(crate::layout::HOME.into()) }]),
                |_| cosmic::Action::App(Message::Tick),
            )
        };
        if self.home.is_some() {
            return away;
        }
        say!("home: show");
        self.home = Some(self.surface);
        self.home_show = 0.0;
        Task::batch([away, self.raise()])
    }

    fn close_home(&mut self) {
        if self.home.is_some() && !self.home_closing && !self.layout.on_stage().is_empty() {
            self.home_closing = true;
            self.back_to_stage = true;
            self.kick();
        }
    }

    fn toggle_menu(&mut self) -> Task<Message> {
        if self.menu.is_some() && !self.menu_closing {
            self.close_menu();
            return Task::none();
        }
        self.menu_closing = false;
        self.card = 0;
        self.sub = Sub::None;
        self.power_armed = false;
        self.kick();
        if self.menu.is_some() {
            return Task::none();
        }
        say!("menu: show");
        self.menu = Some(self.surface);
        self.menu_show = 0.0;
        self.raise()
    }

    fn close_menu(&mut self) {
        if self.menu.is_some() {
            self.menu_closing = true;
            self.kick();
        }
    }

    fn animating(&self) -> bool {
        (self.home.is_some() && (self.home_closing || self.home_show < 1.0))
            || (self.menu.is_some() && (self.menu_closing || self.menu_show < 1.0))
            || self.grow.iter().any(|(k, g)| (g - self.grow_target(k)).abs() > 0.002)
    }

    fn kick(&mut self) {
        self.last_frame = Instant::now();
    }

    fn step(&mut self) -> Task<Message> {
        // a frame later than this is a stall, not a step: jump ahead
        let dt = self.last_frame.elapsed().as_secs_f32();
        let dt = if dt > 0.1 { 1.0 } else { dt };
        self.last_frame = Instant::now();
        let mut tasks = Vec::new();
        if self.home.is_some() {
            if self.home_closing {
                self.home_show = (self.home_show - dt / 0.16).max(0.0);
                if self.home_show <= 0.0 {
                    say!("home: hidden");
                    self.home = None;
                    self.home_closing = false;
                    self.update_pause();
                }
            } else {
                self.home_show = (self.home_show + dt / 0.22).min(1.0);
            }
        }
        if self.menu.is_some() {
            if self.menu_closing {
                self.menu_show = (self.menu_show - dt / 0.14).max(0.0);
                if self.menu_show <= 0.0 {
                    say!("menu: hidden");
                    self.menu = None;
                    self.menu_closing = false;
                }
            } else {
                self.menu_show = (self.menu_show + dt / 0.2).min(1.0);
            }
        }
        tasks.push(self.lower_if_idle());
        let keys: Vec<String> = self.grow.keys().cloned().collect();
        for k in keys {
            let t = self.grow_target(&k);
            if let Some(g) = self.grow.get_mut(&k) {
                ease(g, t, dt, 16.0);
            }
        }
        Task::batch(tasks)
    }

    /// Bring the shell surface above the apps and give it the keyboard.
    fn raise(&mut self) -> Task<Message> {
        if self.surface_up {
            return Task::none();
        }
        self.surface_up = true;
        let id = self.surface;
        Task::batch([
            set_layer(id, Layer::Overlay),
            set_keyboard_interactivity(id, KeyboardInteractivity::Exclusive),
            // a fresh configure, so it draws at once even if it missed frames
            // while it was hidden
            set_size(id, None, None),
        ])
    }

    /// Nothing to show: send the (now transparent) surface below the apps
    /// and give the keyboard back.
    fn lower_if_idle(&mut self) -> Task<Message> {
        if !self.surface_up || self.home.is_some() || self.menu.is_some() {
            return Task::none();
        }
        self.surface_up = false;
        let id = self.surface;
        Task::batch([
            set_keyboard_interactivity(id, KeyboardInteractivity::None),
            set_layer(id, Layer::Background),
        ])
    }

    fn update_pause(&self) {
        let covered = self.home.is_none() && !self.layout.on_stage().is_empty();
        system::pause_wallpaper(covered);
    }

    fn toast(&mut self, s: &str) {
        self.toast = Some((s.to_string(), Instant::now()));
    }

    // ================================================================ niri

    fn niri_event(&mut self, ev: Event) -> Task<Message> {
        let mut new_window = false;
        match ev {
            Event::WorkspacesChanged { workspaces } => self.layout.set_workspaces(&workspaces),
            Event::WindowsChanged { windows } => {
                self.layout.set_windows(windows);
                if !self.started {
                    self.started = true;
                    return Task::batch([self.startup(), self.arrange()]);
                }
            }
            Event::WindowOpenedOrChanged { window } => new_window = self.layout.window_changed(window),
            Event::WindowClosed { id } => {
                say!("window {id} closed");
                self.layout.window_closed(id);
                if self.layout.on_stage().is_empty() {
                    let t = self.open_home();
                    return Task::batch([t, self.arrange()]);
                }
            }
            Event::WindowLayoutsChanged { changes } => self.layout.layouts_changed(changes),
            Event::WindowFocusChanged { id } => {
                self.layout.set_focused(id);
                // the focus moves the shell makes itself are not choices
                let expected = id.is_some_and(|i| self.expect_focus.contains(&i));
                if expected {
                    self.expect_focus.retain(|x| Some(*x) != id);
                } else if !self.arranging && Instant::now() >= self.quiet_until {
                    self.layout.focus_changed(id);
                }
            }
            _ => return Task::none(),
        }
        if new_window {
            self.close_home();
            self.close_menu();
        }
        self.update_pause();
        self.arrange()
    }

    fn startup(&mut self) -> Task<Message> {
        // the startup apps open once per session, not after every restart
        let mark = apps::runtime_dir().join("started");
        let again = mark.exists();
        let _ = std::fs::write(&mark, b"");
        if again || !self.layout.windows.is_empty() {
            // the shell restarted inside a running session
            if !self.layout.on_stage().is_empty() {
                self.close_home();
            }
            return Task::none();
        }
        let ids = apps::id_list("startup", apps::DEFAULT_STARTUP);
        let list: Vec<String> = ids.iter().filter_map(|id| self.installed.get(id)).take(2).map(|a| a.launch()).collect();
        self.layout.startup_left = list.len();
        Task::batch([
            // the second app a moment later, so they open in order
            Task::perform(
                async move {
                    for (i, exec) in list.into_iter().enumerate() {
                        if i > 0 {
                            tokio::time::sleep(Duration::from_millis(900)).await;
                        }
                        niri::run(vec![Action::SpawnSh { command: exec }]).await;
                    }
                },
                |_| cosmic::Action::App(Message::Tick),
            ),
            // an app that never opens must not change what the next one does
            Task::perform(tokio::time::sleep(Duration::from_secs(20)), |_| cosmic::Action::App(Message::StartupOver)),
        ])
    }

    fn arrange(&mut self) -> Task<Message> {
        if self.arranging {
            self.arrange_again = true;
            return Task::none();
        }
        let mut plan = self.layout.plan();
        if plan.is_empty() {
            return Task::none();
        }
        // while Home is up, the apps stay out of sight
        if self.home.is_some() && !self.home_closing && plan.iter().any(|a| matches!(a, Action::FocusWindow { .. })) {
            plan.push(Action::FocusWorkspace { reference: WorkspaceReferenceArg::Name(crate::layout::HOME.into()) });
        }
        self.expect_focus = plan
            .iter()
            .filter_map(|a| match a {
                Action::FocusWindow { id } => Some(*id),
                _ => None,
            })
            .collect();
        self.arranging = true;
        Task::perform(niri::run(plan), |_| cosmic::Action::App(Message::Arranged))
    }

    fn command(&mut self, c: &str) -> Task<Message> {
        // anything other than Super itself cancels a menu about to open
        if c != "super" {
            self.super_seq += 1;
        }
        say!("command {c} (menu {:?} closing {}, home {:?} closing {})", self.menu, self.menu_closing, self.home, self.home_closing);
        // testing only: `chlorum msg key:ArrowDown` presses a key in the shell
        if let Some(k) = c.strip_prefix("key:") {
            if std::env::var_os("CHLORUM_DEBUG").is_some() {
                use keyboard::key::Named;
                let key = match k {
                    "ArrowDown" => keyboard::Key::Named(Named::ArrowDown),
                    "ArrowUp" => keyboard::Key::Named(Named::ArrowUp),
                    "ArrowLeft" => keyboard::Key::Named(Named::ArrowLeft),
                    "ArrowRight" => keyboard::Key::Named(Named::ArrowRight),
                    "Enter" => keyboard::Key::Named(Named::Enter),
                    "Escape" => keyboard::Key::Named(Named::Escape),
                    other => keyboard::Key::Character(other.into()),
                };
                return self.update(Message::Key(self.surface, key, keyboard::Modifiers::empty()));
            }
            return Task::none();
        }
        match c {
            "super" => {
                self.super_seq += 1;
                let seq = self.super_seq;
                if self.menu.is_some() && !self.menu_closing {
                    return self.toggle_menu();
                }
                return Task::perform(tokio::time::sleep(SUPER_DELAY), move |_| {
                    cosmic::Action::App(Message::SuperFire(seq))
                });
            }
            "home" => {
                self.close_menu();
                return self.open_home();
            }
            "lock" => {
                self.close_menu();
                return lock();
            }
            "left" | "right" | "grow" | "shrink" | "swap" | "template" => self.close_menu(),
            _ => {}
        }
        match c {
            "left" => self.layout.focus_side(false),
            "right" => self.layout.focus_side(true),
            "grow" => self.layout.resize(true),
            "shrink" => self.layout.resize(false),
            "swap" => self.layout.swap(),
            "template" => {
                if !self.layout.toggle_two() {
                    self.toast("Open a second app first");
                }
            }
            _ => {}
        }
        self.arrange()
    }

    // ================================================================ home

    fn tiles(&self) -> Vec<Tile> {
        let mut out = Vec::new();
        let mut running_apps: Vec<String> = Vec::new();
        for w in self.layout.apps() {
            let app = w.app_id.as_deref().and_then(|aid| self.installed.values().find(|a| a.owns(aid)));
            if let Some(a) = app {
                running_apps.push(a.id.clone());
            }
            out.push(Tile::Running {
                id: w.id,
                name: app.map(|a| a.name.clone()).or_else(|| w.title.clone()).unwrap_or_else(|| "App".into()),
                icon: app.map(|a| a.icon.clone()).or_else(|| w.app_id.clone()).unwrap_or_default(),
            });
        }
        for id in &self.pinned {
            if running_apps.contains(id) {
                continue;
            }
            if let Some(a) = self.installed.get(id) {
                out.push(Tile::App(a.clone()));
            }
        }
        out.push(Tile::Center);
        out.push(Tile::AllApps);
        out
    }

    fn acts(&self, t: &Tile) -> Vec<HomeAct> {
        let stage = self.layout.on_stage();
        match t {
            Tile::Running { id, .. } => {
                let mut v = vec![HomeAct::Open];
                if !stage.is_empty() && !(stage.len() == 1 && stage[0] == *id) {
                    v.push(HomeAct::Beside);
                }
                v.push(HomeAct::Close);
                v
            }
            Tile::App(_) => {
                if stage.is_empty() { vec![HomeAct::Open] } else { vec![HomeAct::Open, HomeAct::Beside] }
            }
            _ => vec![HomeAct::Open],
        }
    }

    fn grow_target(&self, key: &str) -> f32 {
        if self.page != Page::Main {
            return 0.0;
        }
        let tiles = self.tiles();
        match tiles.get(self.sel.min(tiles.len().saturating_sub(1))) {
            Some(t) if t.key() == key => 1.0,
            _ => 0.0,
        }
    }

    fn do_act(&mut self, tile: Tile, act: HomeAct) -> Task<Message> {
        match (tile, act) {
            (Tile::Center, _) => {
                self.page = Page::Center;
                self.section = 0;
                self.row = None;
            }
            (Tile::AllApps, _) => {
                self.page = Page::AllApps;
                self.all_sel = 0;
            }
            (Tile::Running { id, .. }, HomeAct::Open) => {
                self.layout.bring(id);
                self.close_home_now();
                return self.arrange();
            }
            (Tile::Running { id, .. }, HomeAct::Beside) => {
                self.layout.beside(id);
                self.close_home_now();
                return self.arrange();
            }
            (Tile::Running { id, .. }, HomeAct::Close) => {
                return Task::perform(niri::run(vec![Action::CloseWindow { id: Some(id) }]), |_| {
                    cosmic::Action::App(Message::Tick)
                });
            }
            (Tile::App(a), act) => {
                self.layout.beside_next = act == HomeAct::Beside;
                self.toast(&format!("Opening {}…", a.name));
                // started by niri, so apps live on even if the shell restarts
                return Task::perform(niri::run(vec![Action::SpawnSh { command: a.launch() }]), |_| {
                    cosmic::Action::App(Message::Tick)
                });
            }
        }
        self.kick();
        Task::none()
    }

    fn close_home_now(&mut self) {
        if !self.home_closing {
            self.back_to_stage = true;
        }
        self.home_closing = true;
        self.kick();
    }

    fn home_key(&mut self, key: keyboard::Key, _mods: keyboard::Modifiers) -> Task<Message> {
        use keyboard::key::Named;
        let named = match key.as_ref() {
            keyboard::Key::Named(n) => Some(n),
            _ => None,
        };
        let ch = match key.as_ref() {
            keyboard::Key::Character(c) => c.to_lowercase(),
            _ => String::new(),
        };
        if self.wifi_entry.is_some() && self.page == Page::Center {
            if named == Some(keyboard::key::Named::Escape) && !self.wifi_busy {
                self.wifi_entry = None;
                self.wifi_note = None;
            }
            return Task::none();
        }
        let page = self.page;
        let t = match self.page {
            Page::Main => self.main_key(named, &ch),
            Page::AllApps => self.all_key(named),
            Page::Center => self.center_key(named),
        };
        if named == Some(Named::Escape) && page == Page::Main && self.act.is_none() {
            self.close_home();
        }
        self.kick();
        t
    }

    fn main_key(&mut self, named: Option<keyboard::key::Named>, ch: &str) -> Task<Message> {
        use keyboard::key::Named;
        let tiles = self.tiles();
        let n = tiles.len();
        self.sel = self.sel.min(n.saturating_sub(1));
        let tile = tiles[self.sel].clone();
        let acts = self.acts(&tile);
        match (named, self.act) {
            (Some(Named::ArrowRight), None) => self.sel = (self.sel + 1).min(n - 1),
            (Some(Named::ArrowLeft), None) => self.sel = self.sel.saturating_sub(1),
            (Some(Named::Home), None) => self.sel = 0,
            (Some(Named::End), None) => self.sel = n - 1,
            (Some(Named::ArrowDown), None) => self.act = Some(0),
            (Some(Named::ArrowRight), Some(a)) => self.act = Some((a + 1).min(acts.len() - 1)),
            (Some(Named::ArrowLeft), Some(a)) => self.act = Some(a.saturating_sub(1)),
            (Some(Named::ArrowUp), Some(_)) | (Some(Named::Escape), Some(_)) => self.act = None,
            (Some(Named::Enter), a) => {
                let act = acts[a.unwrap_or(0).min(acts.len() - 1)];
                self.act = None;
                return self.do_act(tile, act);
            }
            _ => {}
        }
        match ch {
            "s" if acts.contains(&HomeAct::Beside) => return self.do_act(tile, HomeAct::Beside),
            "x" if acts.contains(&HomeAct::Close) => return self.do_act(tile, HomeAct::Close),
            _ => {}
        }
        Task::none()
    }

    fn all_apps(&self) -> Vec<App> {
        let mut v: Vec<App> = self.installed.values().filter(|a| a.id != "chlorum-shell").cloned().collect();
        v.sort_by_key(|a| a.name.to_lowercase());
        v
    }

    fn all_key(&mut self, named: Option<keyboard::key::Named>) -> Task<Message> {
        use keyboard::key::Named;
        const COLS: usize = 6;
        let list = self.all_apps();
        let n = list.len().max(1);
        match named {
            Some(Named::ArrowRight) => self.all_sel = (self.all_sel + 1).min(n - 1),
            Some(Named::ArrowLeft) => self.all_sel = self.all_sel.saturating_sub(1),
            Some(Named::ArrowDown) => self.all_sel = (self.all_sel + COLS).min(n - 1),
            Some(Named::ArrowUp) => self.all_sel = self.all_sel.saturating_sub(COLS),
            Some(Named::Escape) => self.page = Page::Main,
            Some(Named::Enter) => {
                if let Some(a) = list.get(self.all_sel) {
                    self.page = Page::Main;
                    return self.do_act(Tile::App(a.clone()), HomeAct::Open);
                }
            }
            _ => {}
        }
        Task::none()
    }

    fn center_key(&mut self, named: Option<keyboard::key::Named>) -> Task<Message> {
        use keyboard::key::Named;
        let section = Section::ALL[self.section];
        let rows = self.rows(section);
        let Some(row) = self.row else {
            match named {
                Some(Named::ArrowDown) => self.section = (self.section + 1).min(Section::ALL.len() - 1),
                Some(Named::ArrowUp) => self.section = self.section.saturating_sub(1),
                Some(Named::ArrowRight) | Some(Named::Enter) if rows > 0 => self.row = Some(0),
                Some(Named::Escape) => self.page = Page::Main,
                _ => {}
            }
            // opening Network looks for Wi-Fi right away
            if Section::ALL[self.section] == Section::Network && self.wifi.is_empty() && !self.wifi_busy {
                self.wifi_busy = true;
                return Task::perform(system::wifi_list(), |v| cosmic::Action::App(Message::WifiList(v)));
            }
            return Task::none();
        };
        let flat = matches!(section, Section::Network | Section::Keyboard) || (section == Section::Apps && self.perm_app.is_none());
        if section == Section::Apps && named == Some(Named::Escape) {
            if let Some(app) = self.perm_app.take() {
                // back to the list, on the same app
                self.row = Some(self.app_list().iter().position(|a| a.id == app).unwrap_or(0));
                return Task::none();
            }
        }
        match named {
            Some(Named::Escape) => self.row = None,
            Some(Named::ArrowLeft) | Some(Named::ArrowRight) if flat => {}
            Some(Named::ArrowDown) => self.row = Some((row + 1).min(rows.max(1) - 1)),
            Some(Named::ArrowUp) => self.row = Some(row.saturating_sub(1)),
            Some(Named::ArrowLeft) => return self.change(section, row, -1),
            Some(Named::ArrowRight) => return self.change(section, row, 1),
            Some(Named::Enter) => return self.change(section, row, 0),
            _ => {}
        }
        Task::none()
    }

    /// ← → change the value on a row, Enter presses it.
    fn change(&mut self, section: Section, row: usize, dir: i32) -> Task<Message> {
        let d = dir as f32;
        match section {
            Section::Wallpaper => {
                let w = &mut self.wallpaper;
                match row {
                    0 => {
                        let i = system::STYLES.iter().position(|s| s.0 == w.style).unwrap_or(0) as i32;
                        let i = (i + dir).clamp(0, system::STYLES.len() as i32 - 1);
                        w.style = system::STYLES[i as usize].0.into();
                    }
                    1 => w.speed = ((w.speed + d * 0.1) * 10.0).round().clamp(0.0, 10.0) / 10.0,
                    2 => w.brightness = ((w.brightness + d * 0.1) * 10.0).round().clamp(1.0, 10.0) / 10.0,
                    3 => w.amount = (w.amount + d).clamp(1.0, 10.0),
                    _ => {
                        let i = system::COLORS.iter().position(|c| c.0.eq_ignore_ascii_case(&w.color)).unwrap_or(0) as i32;
                        let i = (i + dir).clamp(0, system::COLORS.len() as i32 - 1);
                        w.color = system::COLORS[i as usize].0.into();
                    }
                }
                self.wallpaper.save();
            }
            Section::Sound => {
                if row == 0 && dir != 0 {
                    let v = ((self.sound.volume + d * 0.05) * 20.0).round() / 20.0;
                    return Task::perform(system::set_volume(v), |s| cosmic::Action::App(Message::Sound(s)));
                }
                if row == 1 {
                    return Task::perform(system::toggle_mute(), |s| cosmic::Action::App(Message::Sound(s)));
                }
            }
            Section::Updates => {
                if dir == 0 {
                    return self.update_action();
                }
            }
            Section::Network => {
                if dir != 0 {
                    return Task::none();
                }
                if row == 0 {
                    self.wifi_busy = true;
                    self.wifi_note = None;
                    return Task::perform(system::wifi_list(), |v| cosmic::Action::App(Message::WifiList(v)));
                }
                if let Some(w) = self.wifi.get(row - 1).cloned() {
                    if w.active {
                        return Task::none();
                    }
                    self.wifi_note = None;
                    self.wifi_pw.clear();
                    if w.secure {
                        self.wifi_entry = Some(w.ssid);
                        return text_input::focus(self.ids[0].clone());
                    }
                    self.wifi_entry = Some(w.ssid);
                    return self.update(Message::WifiSubmit);
                }
            }
            Section::Keyboard => {
                if dir == 0 {
                    if let Some(l) = crate::setupd::LAYOUTS.get(row) {
                        system::set_keyboard(l.0);
                        self.keyboard = l.0.to_string();
                        self.toast(&format!("Keyboard: {}", l.2));
                    }
                }
            }
            Section::Screen => {
                if dir != 0 {
                    let mut t = self.idle;
                    if row == 0 {
                        t.screen_off = crate::idle::next(crate::idle::SCREEN_OFF, t.screen_off, dir);
                    } else {
                        t.lock = crate::idle::next(crate::idle::LOCK, t.lock, dir);
                    }
                    if t != self.idle {
                        self.idle = t;
                        if let Err(e) = crate::idle::save(t) {
                            self.toast(&format!("Could not save: {e}"));
                        }
                    }
                }
            }
            Section::Apps => {
                let list = self.app_list();
                match self.perm_app.clone() {
                    None => {
                        if dir == 0 {
                            if let Some(a) = list.get(row) {
                                self.denied = crate::sandbox::load();
                                self.perm_app = Some(a.id.clone());
                                self.row = Some(0);
                            }
                        }
                    }
                    Some(app) => {
                        let Some(p) = crate::sandbox::Perm::ALL.get(row).copied() else { return Task::none() };
                        let now = crate::sandbox::allowed(&self.denied, &app, p);
                        // ← turns off, → turns on, Enter flips
                        let want = match dir {
                            -1 => false,
                            1 => true,
                            _ => !now,
                        };
                        if want != now {
                            match crate::sandbox::set(&app, p, want) {
                                Ok(()) => {
                                    self.denied = crate::sandbox::load();
                                    let name = self.installed.get(&app).map(|a| a.name.clone()).unwrap_or(app);
                                    self.toast(&format!("{} {} for {name}. It applies the next time {name} opens.", p.label(), if want { "on" } else { "off" }));
                                }
                                Err(e) => self.toast(&format!("Could not save: {e}")),
                            }
                        }
                    }
                }
            }
            Section::About => {}
        }
        Task::none()
    }

    /// Apps people open, by name.
    fn app_list(&self) -> Vec<App> {
        let mut v: Vec<App> = self.installed.values().filter(|a| !a.id.starts_with("chlorum")).cloned().collect();
        v.sort_by_key(|a| a.name.to_lowercase());
        v
    }

    fn rows(&self, s: Section) -> usize {
        match s {
            Section::Wallpaper => 5,
            Section::Network => 1 + self.wifi.len(),
            Section::Sound => if self.sound.present { 2 } else { 0 },
            Section::Keyboard => crate::setupd::LAYOUTS.len(),
            Section::Screen => 2,
            Section::Apps => if self.perm_app.is_some() { crate::sandbox::Perm::ALL.len() } else { self.app_list().len() },
            Section::Updates => 1,
            Section::About => 0,
        }
    }

    fn update_action(&mut self) -> Task<Message> {
        match self.update {
            Update::Checking | Update::Installing => Task::none(),
            Update::Available => {
                self.update = Update::Installing;
                Task::perform(system::install_update(), |u| cosmic::Action::App(Message::Update(u)))
            }
            _ => {
                self.update = Update::Checking;
                Task::perform(system::check_update(), |u| cosmic::Action::App(Message::Update(u)))
            }
        }
    }

    // ---------------------------------------------------------------- views

    fn home_view(&self) -> Element<'_, Message> {
        let f = ease_out(self.home_show);
        let body = match self.page {
            Page::Main => self.main_view(f),
            Page::AllApps => self.all_view(f),
            Page::Center => self.center_view(f),
        };
        // darker behind the text on the left, the wallpaper shows on the right
        let shade = alpha(Color::from_rgba(0.0, 0.0, 0.0, if self.page == Page::Main { 0.35 } else { 0.8 }), f);
        fill(body, shade).width(Length::Fill).height(Length::Fill).into()
    }

    fn top_bar(&self, f: f32) -> Element<'_, Message> {
        let now = Local::now();
        let clock = widget::Row::new()
            .push(text(format!("{:02}:{:02}", now.hour(), now.minute()), MONO_BOLD, 88.0, alpha(TEXT, f)))
            .push(text(format!(":{:02}", now.second()), MONO_BOLD, 88.0, alpha(YELLOW, f)));
        let date = text(format!("{}  ·  {}", now.format("%A, %-d %B"), zone_label(&now)), SANS, 22.0, alpha(TEXT_2, f));
        let chip = |s: String, strong: bool| -> Element<'_, Message> {
            boxed(text(s, MONO, 15.0, alpha(if strong { YELLOW } else { TEXT }, f)), alpha(Color::from_rgba(0.0, 0.0, 0.0, 0.55), f), alpha(if strong { YELLOW } else { LINE }, f), 1.5)
                .padding([9, 16])
                .into()
        };
        let mut chips = widget::Row::new().spacing(12);
        chips = chips.push(chip(format!("NET · {}", self.network.to_uppercase()), false));
        if self.sound.present {
            let v = if self.sound.muted { "MUTED".to_string() } else { format!("VOL {:.0}", self.sound.volume * 100.0) };
            chips = chips.push(chip(v, false));
        }
        if matches!(self.update, Update::Available | Update::Installed) {
            chips = chips.push(chip("UPDATE READY".into(), true));
        }
        if self.dnd {
            chips = chips.push(chip("DO NOT DISTURB".into(), true));
        }
        chips = chips.push(chip(self.user.to_uppercase(), false));
        widget::Row::new()
            .push(widget::Column::new().push(clock).push(date).spacing(6))
            .push(hfill())
            .push(chips)
            .into()
    }

    fn main_view(&self, f: f32) -> Element<'_, Message> {
        let tiles = self.tiles();
        let sel = self.sel.min(tiles.len().saturating_sub(1));
        let mut ribbon = widget::Row::new().spacing(22).align_y(Alignment::End).height(Length::Fixed(272.0));
        for (i, t) in tiles.iter().enumerate() {
            ribbon = ribbon.push(self.tile_view(i, t, f));
        }
        let tile = &tiles[sel];
        let sub = match tile {
            Tile::Running { id, .. } => {
                if self.layout.on_stage().contains(id) { "On screen now".to_string() } else { "Open, waiting in the background".to_string() }
            }
            Tile::App(_) => "Not open".into(),
            Tile::Center => "Wallpaper, sound, updates".into(),
            Tile::AllApps => format!("{} apps", self.all_apps().len()),
        };
        let mut acts = widget::Row::new().spacing(18);
        for (i, a) in self.acts(tile).into_iter().enumerate() {
            let on = self.act == Some(i);
            let primary = i == 0;
            let bg = if on || (primary && self.act.is_none()) { YELLOW } else { PANEL_2 };
            let fg = if bg == YELLOW { BLACK } else { TEXT };
            let b = fill(
                widget::Row::new()
                    .push(text(a.key(), MONO_BOLD, 15.0, alpha(fg, f)))
                    .push(text(a.label(), SANS_BOLD, 21.0, alpha(fg, f)))
                    .spacing(14)
                    .align_y(Alignment::Center),
                alpha(bg, f),
            )
            .padding([14, 26]);
            acts = acts.push(bracket(b, if on { f } else { 0.0 }, 8.0));
        }

        let logo = widget::icon::from_name("com.chlorum.Logo").size(220);
        let mut col = widget::Column::new()
            .push(widget::Row::new().push(self.top_bar(f)))
            .push(widget::Row::new().push(hfill()).push(if f > 0.05 { Element::from(logo) } else { space(220.0, 220.0) }))
            .push(vfill())
            .push(ribbon)
            .push(space(1.0, 22.0))
            .push(text(tile.name().to_string(), SANS_BOLD, 46.0, alpha(TEXT, f)))
            .push(text(sub, SANS, 21.0, alpha(TEXT_2, f)))
            .push(space(1.0, 16.0))
            .push(acts)
            .push(space(1.0, 28.0));
        if let Some((t, _)) = &self.toast {
            col = col.push(text(t.clone(), MONO, 17.0, alpha(YELLOW, f))).push(space(1.0, 10.0));
        }
        let mut h: Vec<(&str, &str, bool)> = vec![("← →", "Choose", false), ("↓", "More", false), ("ENTER", "Open", true)];
        if !self.layout.on_stage().is_empty() {
            h.push(("S", "Open beside", false));
            h.push(("ESC", "Back to app", false));
        }
        h.push(("SUPER", "System menu", false));
        col.push(hints(&h, f)).padding([48, 72]).into()
    }

    fn tile_view<'a>(&'a self, i: usize, t: &Tile, f: f32) -> Element<'a, Message> {
        let g = self.grow.get(&t.key()).copied().unwrap_or(if i == self.sel { 1.0 } else { 0.0 });
        let size = 150.0 + 90.0 * g;
        let running = matches!(t, Tile::Running { .. });
        let bg = alpha(mix(Color::from_rgba(0.08, 0.08, 0.08, 0.86), YELLOW, g), f);
        let icon: Element<'a, Message> = if f > 0.05 {
            widget::icon::from_name(t.icon().to_string()).size((size * 0.5) as u16).into()
        } else {
            space(size * 0.5, size * 0.5)
        };
        let mut inner = widget::Column::new().align_x(Alignment::Center).width(Length::Fill).height(Length::Fill);
        inner = inner.push(vfill()).push(icon).push(vfill());
        if running {
            // a small bar: this app is open
            inner = inner.push(fill(space(size * 0.3, 4.0), alpha(if g > 0.5 { BLACK } else { YELLOW }, f))).push(space(1.0, 12.0));
        }
        let body = fill(inner, bg).width(Length::Fixed(size)).height(Length::Fixed(size));
        bracket(body, if g > 0.5 && self.act.is_none() { f * g } else { 0.0 }, 9.0)
    }

    fn all_view(&self, f: f32) -> Element<'_, Message> {
        let list = self.all_apps();
        let mut grid = widget::Column::new().spacing(24);
        for (r, chunk) in list.chunks(6).enumerate() {
            let mut row = widget::Row::new().spacing(24);
            for (c, a) in chunk.iter().enumerate() {
                let i = r * 6 + c;
                let on = i == self.all_sel;
                let card = fill(
                    widget::Column::new()
                        .push(widget::icon::from_name(a.icon.clone()).size(72))
                        .push(text(a.name.clone(), SANS_BOLD, 19.0, alpha(if on { BLACK } else { TEXT }, f)))
                        .spacing(16)
                        .align_x(Alignment::Center),
                    alpha(if on { YELLOW } else { PANEL }, f),
                )
                .width(Length::Fixed(240.0))
                .height(Length::Fixed(180.0))
                .align_x(Alignment::Center)
                .align_y(Alignment::Center);
                row = row.push(bracket(card, if on { f } else { 0.0 }, 8.0));
            }
            grid = grid.push(row);
        }
        // keep the chosen row in view
        let scroll = widget::scrollable(grid).height(Length::Fill);
        widget::Column::new()
            .push(text("All apps", SANS_BOLD, 52.0, alpha(TEXT, f)))
            .push(space(1.0, 30.0))
            .push(scroll)
            .push(space(1.0, 20.0))
            .push(hints(&[("← ↑ ↓ →", "Move", false), ("ENTER", "Open", true), ("ESC", "Back", false)], f))
            .padding([48, 72])
            .into()
    }

    fn center_view(&self, f: f32) -> Element<'_, Message> {
        let mut sections = widget::Column::new().spacing(6).width(Length::Fixed(340.0));
        for (i, s) in Section::ALL.iter().enumerate() {
            let on = i == self.section;
            let active = on && self.row.is_none();
            let bg = if on { if active { YELLOW } else { PANEL_2 } } else { Color::TRANSPARENT };
            let fg = if active { BLACK } else if on { YELLOW } else { TEXT_2 };
            sections = sections.push(
                fill(text(s.label(), SANS_BOLD, 26.0, alpha(fg, f)), alpha(bg, f))
                    .padding([18, 28])
                    .width(Length::Fill),
            );
        }
        let content = match Section::ALL[self.section] {
            Section::Wallpaper => self.wallpaper_view(f),
            Section::Network => self.network_view(f),
            Section::Sound => self.sound_view(f),
            Section::Keyboard => self.keyboard_view(f),
            Section::Screen => self.screen_view(f),
            Section::Apps => self.apps_view(f),
            Section::Updates => self.updates_view(f),
            Section::About => self.about_view(f),
        };
        let sec = Section::ALL[self.section];
        let flat = matches!(sec, Section::Network | Section::Keyboard);
        let h: &[(&str, &str, bool)] = if self.wifi_entry.is_some() {
            &[("ENTER", "Connect", true), ("ESC", "Cancel", false)]
        } else if sec == Section::Apps && self.row.is_some() && self.perm_app.is_none() {
            &[("↑ ↓", "App", false), ("ENTER", "What it may use", true), ("ESC", "Sections", false)]
        } else if sec == Section::Apps && self.row.is_some() {
            &[("↑ ↓", "Choose", false), ("← →", "Off · On", true), ("ENTER", "Switch", false), ("ESC", "Apps", false)]
        } else if self.row.is_none() {
            &[("↑ ↓", "Section", false), ("→", "Change", true), ("ESC", "Home", false)]
        } else if flat {
            &[("↑ ↓", "Choose", false), ("ENTER", "Use", true), ("ESC", "Sections", false)]
        } else {
            &[("↑ ↓", "Setting", false), ("← →", "Change", true), ("ENTER", "Press", false), ("ESC", "Sections", false)]
        };
        widget::Column::new()
            .push(text("Center", SANS_BOLD, 52.0, alpha(TEXT, f)))
            .push(space(1.0, 36.0))
            .push(widget::Row::new().push(sections).push(space(70.0, 1.0)).push(content).height(Length::Fill))
            .push(hints(h, f))
            .padding([48, 72])
            .into()
    }

    fn row_box<'a>(&self, i: usize, label: &str, value: String, body: Element<'a, Message>, f: f32) -> Element<'a, Message> {
        let on = self.row == Some(i);
        let head = widget::Row::new()
            .push(text(label.to_string(), SANS_BOLD, 22.0, alpha(if on { TEXT } else { TEXT_2 }, f)))
            .push(hfill())
            .push(text(value, MONO_BOLD, 20.0, alpha(YELLOW, f)))
            .width(Length::Fixed(900.0));
        let col = widget::Column::new().push(head).push(body).spacing(16);
        bracket(fill(col, Color::TRANSPARENT).padding([10, 4]), if on { f } else { 0.0 }, 12.0)
    }

    fn wallpaper_view(&self, f: f32) -> Element<'_, Message> {
        let w = &self.wallpaper;
        let mut styles = widget::Row::new().spacing(14);
        for (id, name) in system::STYLES {
            let on = w.style == *id;
            styles = styles.push(
                fill(text(name.to_string(), SANS_BOLD, 17.0, alpha(if on { BLACK } else { TEXT }, f)), alpha(if on { YELLOW } else { PANEL_2 }, f))
                    .padding([16, 18]),
            );
        }
        let mut colors = widget::Row::new().spacing(18);
        for (hex, _) in system::COLORS {
            let c = parse_hex(hex);
            let on = w.color.eq_ignore_ascii_case(hex);
            let sw = fill(space(52.0, 52.0), alpha(c, f));
            colors = colors.push(boxed(sw, Color::TRANSPARENT, alpha(if on { TEXT } else { Color::TRANSPARENT }, f), 3.0).padding(5));
        }
        let style_name = system::STYLES.iter().find(|s| s.0 == w.style).map(|s| s.1).unwrap_or("").to_string();
        let color_name = system::COLORS.iter().find(|c| c.0.eq_ignore_ascii_case(&w.color)).map(|c| c.1).unwrap_or("").to_string();
        widget::Column::new()
            .push(self.row_box(0, "Style", style_name, styles.into(), f))
            .push(self.row_box(1, "Speed", format!("{:.0}%", w.speed * 100.0), slider(w.speed, 900.0, self.row == Some(1), f), f))
            .push(self.row_box(2, "Brightness", format!("{:.0}%", w.brightness * 100.0), slider(w.brightness, 900.0, self.row == Some(2), f), f))
            .push(self.row_box(3, "Amount", format!("{:.0}", w.amount), slider((w.amount - 1.0) / 9.0, 900.0, self.row == Some(3), f), f))
            .push(self.row_box(4, "Colour", color_name, colors.into(), f))
            .spacing(26)
            .into()
    }

    /// A one-line choice in a list: chosen = yellow.
    fn list_row<'a>(&self, i: usize, name: String, detail: String, f: f32) -> Element<'a, Message> {
        let on = self.row == Some(i);
        fill(
            widget::Row::new()
                .push(text(name, SANS_BOLD, 21.0, alpha(if on { BLACK } else { TEXT }, f)))
                .push(hfill())
                .push(text(detail, MONO, 15.0, alpha(if on { BLACK } else { TEXT_2 }, f)))
                .align_y(Alignment::Center),
            alpha(if on { YELLOW } else { PANEL }, f),
        )
        .padding([14, 22])
        .width(Length::Fixed(900.0))
        .into()
    }

    fn network_view(&self, f: f32) -> Element<'_, Message> {
        let mut col = widget::Column::new().spacing(8);
        col = col
            .push(text(format!("Now: {}", if self.network.is_empty() { "Offline" } else { &self.network }), SANS_BOLD, 28.0, alpha(TEXT, f)))
            .push(space(1.0, 14.0));
        let search = if self.wifi_busy && self.wifi_entry.is_none() { "Searching…" } else { "Search for Wi-Fi again" };
        col = col.push(self.list_row(0, search.into(), "ENTER".into(), f));
        for (i, w) in self.wifi.iter().enumerate() {
            let detail = if w.active {
                "CONNECTED".to_string()
            } else {
                format!("{}{}%", if w.secure { "LOCKED · " } else { "" }, w.signal)
            };
            col = col.push(self.list_row(i + 1, w.ssid.clone(), detail, f));
        }
        if let Some(ssid) = &self.wifi_entry {
            let input = text_input::text_input("Wi-Fi password", self.wifi_pw.as_str())
                .id(self.ids[0].clone())
                .password()
                .style(field_style())
                .size(24)
                .padding([16, 20])
                .width(Length::Fixed(640.0))
                .on_input(Message::WifiInput)
                .on_submit(|_| Message::WifiSubmit);
            col = col
                .push(space(1.0, 12.0))
                .push(text(format!("PASSWORD FOR {}", ssid.to_uppercase()), MONO, 15.0, alpha(TEXT_2, f)))
                .push(bracket(input, f, 8.0));
        }
        if self.wifi_busy && self.wifi_entry.is_some() {
            col = col.push(crate::atom::Atom::view(64.0, self.started_at));
        }
        if let Some(n) = &self.wifi_note {
            col = col.push(text(n.clone(), SANS_BOLD, 20.0, alpha(YELLOW, f)));
        }
        widget::scrollable(col).height(Length::Fill).into()
    }

    fn screen_view(&self, f: f32) -> Element<'_, Message> {
        use crate::idle::{LOCK, SCREEN_OFF, label};
        let choice = |list: &'static [u32], now: u32, on: bool| -> Element<'_, Message> {
            let mut r = widget::Row::new().spacing(10);
            for v in list {
                let sel = *v == now;
                r = r.push(
                    fill(text(label(*v), SANS_BOLD, 16.0, alpha(if sel { BLACK } else if on { TEXT } else { TEXT_2 }, f)), alpha(if sel { YELLOW } else { PANEL_2 }, f))
                        .padding([12, 14]),
                );
            }
            r.into()
        };
        widget::Column::new()
            .push(text("When nobody uses the computer. A video or a call keeps it awake.", SANS, 20.0, alpha(TEXT_2, f)))
            .push(self.row_box(0, "Turn the screen off after", label(self.idle.screen_off), choice(SCREEN_OFF, self.idle.screen_off, self.row == Some(0)), f))
            .push(self.row_box(1, "Lock after", label(self.idle.lock), choice(LOCK, self.idle.lock, self.row == Some(1)), f))
            .spacing(26)
            .into()
    }

    fn apps_view(&self, f: f32) -> Element<'_, Message> {
        use crate::sandbox::{Perm, allowed};
        let mut col = widget::Column::new().spacing(8);
        if let Some(app) = &self.perm_app {
            let name = self.installed.get(app).map(|a| a.name.clone()).unwrap_or_else(|| app.clone());
            col = col
                .push(text(name, SANS_BOLD, 34.0, alpha(TEXT, f)))
                .push(text("What it may use. A change applies the next time it opens.", SANS, 20.0, alpha(TEXT_2, f)))
                .push(space(1.0, 14.0));
            for (i, p) in Perm::ALL.iter().enumerate() {
                let on = allowed(&self.denied, app, *p);
                let sel = self.row == Some(i);
                let fg = if sel { BLACK } else { TEXT };
                let state = fill(text(if on { "ON" } else { "OFF" }, MONO_BOLD, 16.0, alpha(if sel { YELLOW } else if on { BLACK } else { TEXT }, f)), alpha(if sel { BLACK } else if on { YELLOW } else { PANEL_2 }, f))
                    .padding([6, 14]);
                let row = fill(
                    widget::Row::new()
                        .push(
                            widget::Column::new()
                                .push(text(p.label(), SANS_BOLD, 21.0, alpha(fg, f)))
                                .push(text(if on { "Allowed." } else { p.off_means() }, SANS, 16.0, alpha(if sel { BLACK } else { TEXT_2 }, f)))
                                .spacing(4),
                        )
                        .push(hfill())
                        .push(state)
                        .align_y(Alignment::Center),
                    alpha(if sel { YELLOW } else { PANEL }, f),
                )
                .padding([14, 22])
                .width(Length::Fixed(900.0));
                col = col.push(row);
            }
            return col.into();
        }
        col = col
            .push(text("Choose an app to see what it may use: internet, camera, sound, your files.", SANS, 20.0, alpha(TEXT_2, f)))
            .push(space(1.0, 10.0));
        let list = self.app_list();
        // the chosen app stays in view: show a window of the list around it
        const SHOWN: usize = 9;
        let sel = self.row.unwrap_or(0);
        let start = sel.saturating_sub(SHOWN / 2).min(list.len().saturating_sub(SHOWN));
        for (i, a) in list.iter().enumerate().skip(start).take(SHOWN) {
            let off: Vec<&str> = Perm::ALL.iter().filter(|p| !allowed(&self.denied, &a.id, **p)).map(|p| p.key()).collect();
            let detail = if off.is_empty() { "ALL ALLOWED".to_string() } else { format!("{} OFF", off.join(" · ").to_uppercase()) };
            col = col.push(self.list_row(i, a.name.clone(), detail, f));
        }
        if list.len() > SHOWN {
            col = col.push(text(format!("{} of {}", sel + 1, list.len()), MONO, 14.0, alpha(TEXT_2, f)));
        }
        col.into()
    }

    fn keyboard_view(&self, f: f32) -> Element<'_, Message> {
        let mut col = widget::Column::new().spacing(8);
        col = col
            .push(text("Alt + Shift switches between this layout and English (US).", SANS, 20.0, alpha(TEXT_2, f)))
            .push(space(1.0, 10.0));
        for (i, (id, _, name)) in crate::setupd::LAYOUTS.iter().enumerate() {
            let detail = if *id == self.keyboard { "IN USE".to_string() } else { id.to_uppercase() };
            col = col.push(self.list_row(i, name.to_string(), detail, f));
        }
        widget::scrollable(col).height(Length::Fill).into()
    }

    fn sound_view(&self, f: f32) -> Element<'_, Message> {
        if !self.sound.present {
            return text("No speakers or headphones found.", SANS, 24.0, alpha(TEXT_2, f));
        }
        let mute = fill(
            text(if self.sound.muted { "Sound is off" } else { "Sound is on" }, SANS_BOLD, 20.0, alpha(TEXT, f)),
            alpha(PANEL_2, f),
        )
        .padding([16, 22]);
        widget::Column::new()
            .push(self.row_box(0, "Volume", format!("{:.0}%", self.sound.volume * 100.0), slider(self.sound.volume, 900.0, self.row == Some(0), f), f))
            .push(self.row_box(1, "Mute", if self.sound.muted { "ON".into() } else { "OFF".into() }, mute.into(), f))
            .spacing(26)
            .into()
    }

    fn updates_view(&self, f: f32) -> Element<'_, Message> {
        let label = match self.update {
            Update::Available => "Install now",
            Update::Checking | Update::Installing => "Please wait…",
            _ => "Check for updates",
        };
        let b = fill(text(label, SANS_BOLD, 21.0, alpha(BLACK, f)), alpha(YELLOW, f)).padding([16, 26]);
        widget::Column::new()
            .push(text(format!("Chlorum {}", system::version()), SANS_BOLD, 30.0, alpha(TEXT, f)))
            .push(text(self.update.text(), SANS, 22.0, alpha(TEXT_2, f)))
            .push(space(1.0, 20.0))
            .push(self.row_box(0, "Updates", String::new(), b.into(), f))
            .spacing(10)
            .into()
    }

    fn about_view(&self, f: f32) -> Element<'_, Message> {
        let line = |k: &str, v: String| -> Element<'_, Message> {
            widget::Row::new()
                .push(text(k.to_string(), MONO, 18.0, alpha(TEXT_2, f)).apply_width(260.0))
                .push(text(v, SANS_BOLD, 22.0, alpha(TEXT, f)))
                .align_y(Alignment::Center)
                .into()
        };
        widget::Column::new()
            .push(widget::icon::from_name("com.chlorum.Logo").size(200))
            .push(space(1.0, 20.0))
            .push(line("VERSION", system::version()))
            .push(line("COMPUTER", system::computer_name()))
            .push(line("PROFILE", self.user.clone()))
            .push(line("MOTTO", "Order by default, freedom by choice".into()))
            .spacing(14)
            .into()
    }

    // ================================================================ system menu

    fn cards(&self) -> Vec<Card> {
        let mut v = vec![Card::Layout, Card::Apps, Card::Notes];
        if matches!(self.update, Update::Available | Update::Installing | Update::Installed) {
            v.push(Card::Update);
        }
        v.extend([Card::Sound, Card::Home, Card::Center, Card::Power]);
        v
    }

    fn menu_key(&mut self, key: keyboard::Key) -> Task<Message> {
        use keyboard::key::Named;
        let named = match key.as_ref() {
            keyboard::Key::Named(n) => Some(n),
            _ => None,
        };
        let ch = match key.as_ref() {
            keyboard::Key::Character(c) => c.to_lowercase(),
            _ => String::new(),
        };
        self.kick();
        let cards = self.cards();
        self.card = self.card.min(cards.len() - 1);
        match self.sub {
            Sub::Apps(i) => {
                let list: Vec<u64> = self.layout.apps().iter().map(|w| w.id).collect();
                let n = list.len();
                match named {
                    Some(Named::ArrowRight) if n > 0 => self.sub = Sub::Apps((i + 1).min(n - 1)),
                    Some(Named::ArrowLeft) => self.sub = Sub::Apps(i.saturating_sub(1)),
                    Some(Named::Escape) | Some(Named::ArrowUp) => self.sub = Sub::None,
                    Some(Named::Enter) => {
                        if let Some(id) = list.get(i) {
                            self.layout.bring(*id);
                            self.close_menu();
                            self.close_home_now_if_open();
                            return self.arrange();
                        }
                    }
                    _ => {}
                }
                if ch == "s" {
                    if let Some(id) = list.get(i) {
                        self.layout.beside(*id);
                        self.close_menu();
                        self.close_home_now_if_open();
                        return self.arrange();
                    }
                }
                if ch == "x" {
                    if let Some(id) = list.get(i) {
                        let id = *id;
                        if i + 1 >= n {
                            self.sub = if n > 1 { Sub::Apps(i - 1) } else { Sub::None };
                        }
                        return Task::perform(niri::run(vec![Action::CloseWindow { id: Some(id) }]), |_| {
                            cosmic::Action::App(Message::Tick)
                        });
                    }
                }
                return Task::none();
            }
            Sub::Notes(i) => {
                let n = self.notes.len();
                match named {
                    Some(Named::ArrowRight) if n > 0 => self.sub = Sub::Notes((i + 1).min(n - 1)),
                    Some(Named::ArrowLeft) => self.sub = Sub::Notes(i.saturating_sub(1)),
                    Some(Named::Escape) | Some(Named::ArrowUp) => self.sub = Sub::None,
                    _ => {}
                }
                if ch == "x" && i < n {
                    self.notes.remove(i);
                    self.sub = if self.notes.is_empty() { Sub::None } else { Sub::Notes(i.min(self.notes.len() - 1)) };
                }
                return Task::none();
            }
            Sub::Power(i) => {
                match named {
                    Some(Named::ArrowRight) => {
                        self.sub = Sub::Power((i + 1).min(4));
                        self.power_armed = false;
                    }
                    Some(Named::ArrowLeft) => {
                        self.sub = Sub::Power(i.saturating_sub(1));
                        self.power_armed = false;
                    }
                    Some(Named::Escape) | Some(Named::ArrowUp) => self.sub = Sub::None,
                    Some(Named::Enter) => {
                        if i == 0 {
                            self.close_menu();
                            return lock();
                        }
                        let i = i - 1;
                        if i == 3 {
                            self.sub = Sub::None;
                        } else if self.power_armed {
                            if Power::ALL[i] == Power::LogOut {
                                return Task::perform(niri::run(vec![Action::Quit { skip_confirmation: true }]), |_| {
                                    cosmic::Action::App(Message::Tick)
                                });
                            }
                            Power::ALL[i].run();
                        } else {
                            // nothing switches off by accident: Enter twice
                            self.power_armed = true;
                        }
                    }
                    _ => {}
                }
                return Task::none();
            }
            Sub::Sound => {
                match named {
                    Some(Named::ArrowRight) | Some(Named::ArrowLeft) => {
                        let d = if named == Some(Named::ArrowRight) { 0.05 } else { -0.05 };
                        let v = ((self.sound.volume + d) * 20.0).round() / 20.0;
                        self.sound.volume = v.clamp(0.0, 1.0);
                        return Task::perform(system::set_volume(v), |s| cosmic::Action::App(Message::Sound(s)));
                    }
                    Some(Named::Enter) | Some(Named::Escape) | Some(Named::ArrowUp) => self.sub = Sub::None,
                    _ => {}
                }
                return Task::none();
            }
            Sub::None => {}
        }
        match named {
            Some(Named::ArrowRight) => self.card = (self.card + 1).min(cards.len() - 1),
            Some(Named::ArrowLeft) => self.card = self.card.saturating_sub(1),
            Some(Named::Escape) => self.close_menu(),
            Some(Named::Enter) | Some(Named::ArrowDown) => return self.press_card(cards[self.card]),
            Some(Named::F10) => self.sub = Sub::Power(1),
            _ => {}
        }
        match ch.as_str() {
            "d" => self.toggle_dnd(),
            "h" => return self.press_card(Card::Home),
            "c" => return self.press_card(Card::Center),
            "l" => {
                self.close_menu();
                return lock();
            }
            _ => {}
        }
        Task::none()
    }

    fn close_home_now_if_open(&mut self) {
        if self.home.is_some() {
            self.close_home_now();
        }
    }

    fn press_card(&mut self, card: Card) -> Task<Message> {
        match card {
            Card::Layout => {
                if !self.layout.toggle_two() {
                    self.toast("Open a second app first");
                }
                return self.arrange();
            }
            Card::Apps => {
                if !self.layout.apps().is_empty() {
                    self.sub = Sub::Apps(0);
                }
            }
            Card::Update => return self.update_action(),
            Card::Notes => {
                if !self.notes.is_empty() {
                    self.sub = Sub::Notes(0);
                }
            }
            Card::Sound => self.sub = Sub::Sound,
            Card::Home => {
                self.close_menu();
                return self.open_home();
            }
            Card::Center => {
                self.close_menu();
                let t = self.open_home();
                self.page = Page::Center;
                self.section = 0;
                self.row = None;
                return t;
            }
            Card::Power => {
                self.sub = Sub::Power(1);
                self.power_armed = false;
            }
        }
        Task::none()
    }

    fn app_name(&self, id: u64) -> String {
        let Some(w) = self.layout.windows.get(&id) else { return "App".into() };
        w.app_id
            .as_deref()
            .and_then(|aid| self.installed.values().find(|a| a.owns(aid)))
            .map(|a| a.name.clone())
            .or_else(|| w.title.clone())
            .unwrap_or_else(|| "App".into())
    }

    fn app_icon(&self, id: u64) -> String {
        let Some(w) = self.layout.windows.get(&id) else { return String::new() };
        w.app_id
            .as_deref()
            .and_then(|aid| self.installed.values().find(|a| a.owns(aid)))
            .map(|a| a.icon.clone())
            .or_else(|| w.app_id.clone())
            .unwrap_or_default()
    }

    fn menu_view(&self) -> Element<'_, Message> {
        let f = ease_out(self.menu_show);
        let now = Local::now();
        let what = match self.layout.stage() {
            Stage::Empty => "Home".to_string(),
            Stage::One(a) => format!("{}  ·  one app", self.app_name(a)),
            Stage::Two { left, right, .. } => format!("{}  +  {}", self.app_name(left), self.app_name(right)),
        };
        let head = widget::Row::new()
            .push(if f > 0.05 { Element::from(widget::icon::from_name("com.chlorum.Logo").size(84)) } else { space(84.0, 84.0) })
            .push(space(22.0, 1.0))
            .push(text(what, SANS_BOLD, 26.0, alpha(TEXT, f)))
            .push(hfill())
            .push(text(format!("{:02}:{:02}", now.hour(), now.minute()), MONO_BOLD, 34.0, alpha(TEXT, f)))
            .push(text(format!(":{:02}", now.second()), MONO_BOLD, 34.0, alpha(YELLOW, f)))
            .align_y(Alignment::Center);

        let cards = self.cards();
        let mut row = widget::Row::new().spacing(22).align_y(Alignment::Start);
        for (i, c) in cards.iter().enumerate() {
            row = row.push(self.card_view(*c, i == self.card && self.sub == Sub::None, i == self.card, f));
        }

        let sub: Element<'_, Message> = match self.sub {
            Sub::Apps(sel) => {
                let mut r = widget::Row::new().spacing(18);
                for (i, w) in self.layout.apps().iter().enumerate() {
                    let on = i == sel;
                    let tile = fill(
                        widget::Column::new()
                            .push(widget::icon::from_name(self.app_icon(w.id)).size(56))
                            .push(text(self.app_name(w.id), SANS_BOLD, 17.0, alpha(if on { BLACK } else { TEXT }, f)))
                            .spacing(10)
                            .align_x(Alignment::Center),
                        alpha(if on { YELLOW } else { PANEL_2 }, f),
                    )
                    .width(Length::Fixed(190.0))
                    .height(Length::Fixed(130.0))
                    .align_x(Alignment::Center)
                    .align_y(Alignment::Center);
                    r = r.push(bracket(tile, if on { f } else { 0.0 }, 8.0));
                }
                r.into()
            }
            Sub::Power(sel) => {
                let mut r = widget::Row::new().spacing(18);
                let labels = ["Lock", "Log out", "Restart", "Shut down", "Cancel"];
                for (i, l) in labels.iter().enumerate() {
                    let on = i == sel;
                    let armed = on && self.power_armed && (1..=3).contains(&i);
                    let label = if armed { format!("Press Enter to {}", l.to_lowercase()) } else { l.to_string() };
                    let bg = if armed { RED } else if on { YELLOW } else { PANEL_2 };
                    let b = fill(text(label, SANS_BOLD, 21.0, alpha(if on { BLACK } else { TEXT }, f)), alpha(bg, f)).padding([18, 28]);
                    r = r.push(bracket(b, if on { f } else { 0.0 }, 8.0));
                }
                r.into()
            }
            Sub::Notes(sel) => {
                let mut r = widget::Row::new().spacing(18);
                for (i, n) in self.notes.iter().enumerate().take(6) {
                    let on = i == sel;
                    let fg = if on { BLACK } else { TEXT };
                    let tile = fill(
                        widget::Column::new()
                            .push(text(n.app.to_uppercase(), MONO, 12.0, alpha(if on { BLACK } else { TEXT_2 }, f)))
                            .push(text(clip(&n.summary, 28), SANS_BOLD, 17.0, alpha(fg, f)))
                            .push(text(clip(&n.body, 60), SANS, 14.0, alpha(fg, f)))
                            .spacing(6),
                        alpha(if on { YELLOW } else { PANEL_2 }, f),
                    )
                    .padding(16)
                    .width(Length::Fixed(260.0))
                    .height(Length::Fixed(130.0));
                    r = r.push(bracket(tile, if on { f } else { 0.0 }, 8.0));
                }
                r.into()
            }
            _ => space(1.0, 1.0),
        };

        let h: &[(&str, &str, bool)] = match self.sub {
            Sub::None => &[("← →", "Move", false), ("ENTER", "Choose", true), ("H", "Home", false), ("C", "Center", false), ("L", "Lock", false), ("D", "Quiet", false), ("F10", "Power", false), ("SUPER", "Close", false)],
            Sub::Apps(_) => &[("← →", "App", false), ("ENTER", "Show", true), ("S", "Show beside", false), ("X", "Close app", false), ("ESC", "Back", false)],
            Sub::Power(_) => &[("← →", "Choose", false), ("ENTER", "Twice to confirm", true), ("ESC", "Back", false)],
            Sub::Sound => &[("← →", "Volume", true), ("ENTER", "Done", false)],
            Sub::Notes(_) => &[("← →", "Move", false), ("X", "Dismiss", true), ("ESC", "Back", false)],
        };

        let mut sheet_col = widget::Column::new().push(head).push(space(1.0, 30.0)).push(row).push(space(1.0, 22.0)).push(sub);
        if let Some((t, _)) = &self.toast {
            sheet_col = sheet_col.push(space(1.0, 12.0)).push(text(t.clone(), MONO, 17.0, alpha(YELLOW, f)));
        }
        sheet_col = sheet_col.push(vfill()).push(hints(h, f));
        let sheet = widget::Column::new()
            .push(fill(space(1.0, 3.0), alpha(YELLOW, f)).width(Length::Fill))
            .push(fill(sheet_col.padding([34, 72]), alpha(Color::from_rgba(0.035, 0.035, 0.035, 0.97), f)).width(Length::Fill).height(Length::Fixed(560.0)));
        // rises from the bottom edge
        let drawer = widget::container(sheet)
            .height(Length::Fixed(563.0 * f))
            .width(Length::Fill)
            .align_y(Alignment::Start)
            .clip(true);
        let content = widget::Column::new().push(vfill()).push(drawer);
        fill(content, alpha(Color::from_rgba(0.0, 0.0, 0.0, 0.55), f)).width(Length::Fill).height(Length::Fill).into()
    }

    fn card_view(&self, c: Card, focused: bool, current: bool, f: f32) -> Element<'_, Message> {
        let (label, value, extra): (&str, String, Element<'_, Message>) = match c {
            Card::Layout => {
                let two = matches!(self.layout.stage(), Stage::Two { .. });
                let pic = |is_two: bool, sel: bool| -> Element<'_, Message> {
                    let line = alpha(
                        match (focused, sel) {
                            (true, true) => BLACK,
                            (true, false) => Color::from_rgba(0.0, 0.0, 0.0, 0.35),
                            (false, true) => YELLOW,
                            (false, false) => MUTED,
                        },
                        f,
                    );
                    let inner: Element<'_, Message> = if is_two {
                        widget::Row::new()
                            .push(space(44.0, 40.0))
                            .push(fill(space(3.0, 40.0), line))
                            .into()
                    } else {
                        space(80.0, 40.0)
                    };
                    boxed(inner, Color::TRANSPARENT, line, 3.0).into()
                };
                let pics = widget::Row::new().push(pic(false, !two)).push(pic(true, two)).spacing(14);
                let v = match self.layout.stage() {
                    Stage::Two { ratio, .. } => format!("Two apps · {:.0}:{:.0}", ratio * 100.0, (1.0 - ratio) * 100.0),
                    Stage::One(_) => "One app".into(),
                    Stage::Empty => "Nothing open".into(),
                };
                ("LAYOUT", v, pics.into())
            }
            Card::Apps => {
                let n = self.layout.apps().len();
                ("OPEN APPS", if n == 0 { "None".into() } else { format!("{n} open") }, text("SWITCH · CLOSE", MONO, 13.0, alpha(TEXT_2, f)))
            }
            Card::Notes => {
                let n = self.notes.len();
                let last = if self.dnd {
                    "DO NOT DISTURB · D".to_string()
                } else {
                    self.notes.first().map(|n| clip(&n.summary, 20).to_uppercase()).unwrap_or_else(|| "NOTHING NEW · D QUIET".into())
                };
                ("NOTIFICATIONS", if n == 0 { "None".into() } else { format!("{n} new") }, text(last, MONO, 12.0, alpha(if self.dnd { YELLOW } else { TEXT_2 }, f)))
            }
            Card::Update => ("UPDATE", "New version".into(), text(self.update.text().to_uppercase(), MONO, 12.0, alpha(TEXT_2, f))),
            Card::Sound => {
                let v = if self.sound.muted { "Off".to_string() } else { format!("{:.0}", self.sound.volume * 100.0) };
                ("SOUND", v, slider(self.sound.volume, 136.0, self.sub == Sub::Sound, f))
            }
            Card::Home => ("HOME", "Home".into(), text("H", MONO, 13.0, alpha(TEXT_2, f))),
            Card::Center => ("CENTER", "Settings".into(), text("C", MONO, 13.0, alpha(TEXT_2, f))),
            Card::Power => ("POWER", "Lock · off".into(), text("L · F10", MONO, 13.0, alpha(TEXT_2, f))),
        };
        let strong = focused;
        let fg = if strong { BLACK } else { TEXT };
        let lab = if strong { BLACK } else { TEXT_2 };
        let width = if c == Card::Layout { 290.0 } else { 180.0 };
        let body = widget::Column::new()
            .push(text(label, MONO, 14.0, alpha(lab, f)))
            .push(vfill())
            .push(text(value, SANS_BOLD, 23.0, alpha(fg, f)))
            .push(space(1.0, 12.0))
            .push(extra)
            .padding(22)
            .width(Length::Fixed(width))
            .height(Length::Fixed(220.0));
        let bg = if strong { YELLOW } else if current { PANEL_2 } else { PANEL };
        bracket(fill(body, alpha(bg, f)), if current { f } else { 0.0 }, 9.0)
    }
}

trait ApplyWidth<'a> {
    fn apply_width(self, w: f32) -> Element<'a, Message>;
}

impl<'a> ApplyWidth<'a> for Element<'a, Message> {
    fn apply_width(self, w: f32) -> Element<'a, Message> {
        widget::container(self).width(Length::Fixed(w)).into()
    }
}

impl Shell {
    // ================================================================ notifications

    fn toggle_dnd(&mut self) {
        self.dnd = !self.dnd;
        let p = dnd_path();
        let r = if self.dnd {
            p.parent().map(std::fs::create_dir_all).transpose().and_then(|_| std::fs::write(&p, b""))
        } else {
            std::fs::remove_file(&p).or_else(|e| if e.kind() == std::io::ErrorKind::NotFound { Ok(()) } else { Err(e) })
        };
        match r {
            Ok(()) => self.toast(if self.dnd { "Do not disturb: notifications wait here, quietly." } else { "Notifications pop up again." }),
            Err(e) => self.toast(&format!("Could not save: {e}")),
        }
    }

    fn show_note(&mut self, n: Note) -> Task<Message> {
        self.notes.retain(|x| x.id != n.id);
        self.notes.insert(0, n);
        self.notes.truncate(20);
        if self.dnd {
            // kept in the menu, no pop-up
            return Task::none();
        }
        let shown = self.note_surface.is_some();
        self.note_surface = Some((self.note_id, Instant::now()));
        if shown { Task::none() } else { Task::batch([set_layer(self.note_id, Layer::Overlay), set_size(self.note_id, Some(480), Some(132))]) }
    }

    fn note_view(&self) -> Element<'_, Message> {
        let Some(n) = self.notes.first() else { return space(1.0, 1.0) };
        let icon: Element<'_, Message> = if n.icon.is_empty() {
            widget::icon::from_name("com.chlorum.Logo").size(56).into()
        } else if n.icon.starts_with('/') {
            widget::icon::icon(widget::icon::from_path(n.icon.clone().into())).size(56).into()
        } else {
            widget::icon::from_name(n.icon.clone()).size(56).into()
        };
        let body = widget::Row::new()
            .push(icon)
            .push(
                widget::Column::new()
                    .push(text(n.app.to_uppercase(), MONO, 12.0, TEXT_2))
                    .push(text(clip(&n.summary, 40), SANS_BOLD, 19.0, TEXT))
                    .push(text(clip(&n.body, 90), SANS, 15.0, TEXT_2))
                    .spacing(4),
            )
            .spacing(18)
            .align_y(Alignment::Center);
        widget::Column::new()
            .push(fill(space(1.0, 3.0), YELLOW).width(Length::Fill))
            .push(fill(body, Color::from_rgba(0.05, 0.05, 0.05, 0.96)).padding([16, 20]).width(Length::Fill).height(Length::Fill))
            .into()
    }

}

/// The lock screen is its own small program, started by niri: it keeps the
/// screen locked even if the shell restarts.
fn lock() -> Task<Message> {
    if std::env::var_os("NIRI_SOCKET").is_none() {
        if let Ok(me) = std::env::current_exe() {
            let _ = std::process::Command::new(me).arg("lock").spawn();
        }
        return Task::none();
    }
    Task::perform(niri::run(vec![Action::SpawnSh { command: "chlorum lock".into() }]), |_| cosmic::Action::App(Message::Tick))
}

fn clip(s: &str, n: usize) -> String {
    let s = s.replace('\n', " ");
    if s.chars().count() <= n { s } else { s.chars().take(n - 1).collect::<String>() + "…" }
}

fn ease_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

fn parse_hex(s: &str) -> Color {
    let v = u32::from_str_radix(s.trim_start_matches('#'), 16).unwrap_or(0xFFD60A);
    Color::from_rgb8((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

fn zone_label(now: &chrono::DateTime<Local>) -> String {
    let zone = std::env::var("TZ")
        .ok()
        .filter(|z| !z.is_empty())
        .or_else(|| {
            std::fs::read_link("/etc/localtime").ok().and_then(|p| {
                let s = p.to_string_lossy().to_string();
                s.split("zoneinfo/").nth(1).map(str::to_string)
            })
        })
        .unwrap_or_else(|| "UTC".into());
    let city = zone.rsplit('/').next().unwrap_or(&zone).replace('_', " ");
    let off = now.offset().fix().local_minus_utc() / 60;
    let sign = if off < 0 { '-' } else { '+' };
    if off % 60 == 0 {
        format!("{city} (UTC{sign}{})", off.abs() / 60)
    } else {
        format!("{city} (UTC{sign}{}:{:02})", off.abs() / 60, off.abs() % 60)
    }
}
