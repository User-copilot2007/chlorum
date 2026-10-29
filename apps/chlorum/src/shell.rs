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
    // the screen's size first, then everything is drawn for it
    crate::display::apply_now();
    let settings = Settings::default().scale_factor(crate::ui::screen_zoom())
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
    Bluetooth,
    Sound,
    Keyboard,
    Screen,
    Home,
    Apps,
    GetApps,
    Updates,
    Shortcuts,
    About,
}

impl Section {
    const ALL: [Section; 12] = [
        Section::Wallpaper,
        Section::Network,
        Section::Bluetooth,
        Section::Sound,
        Section::Keyboard,
        Section::Screen,
        Section::Home,
        Section::Apps,
        Section::GetApps,
        Section::Updates,
        Section::Shortcuts,
        Section::About,
    ];
    fn label(self) -> &'static str {
        match self {
            Section::Wallpaper => "Wallpaper",
            Section::Network => "Network",
            Section::Bluetooth => "Bluetooth",
            Section::Sound => "Sound",
            Section::Keyboard => "Keyboard",
            Section::Screen => "Screen",
            Section::Home => "Home",
            Section::Apps => "Apps",
            Section::GetApps => "Get apps",
            Section::Updates => "Updates",
            Section::Shortcuts => "Shortcuts",
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
    /// a window asked to close is still there after a while
    NotClosing(u64),
    /// something to show for a moment
    Said(String),
    Devices(system::Devices),
    SoundNote(Option<String>),
    /// is the Wi-Fi radio on (None: no Wi-Fi)
    WifiOn(Option<bool>),
    WifiSwitched(Result<String, String>, Option<bool>, Vec<Wifi>, String),
    WifiDone(Result<(), String>),
    WifiInput(String),
    WifiSubmit,
    Note(notify::Event),
    /// the screens as niri drives them now
    Screens(Vec<crate::display::Screen>),
    Bt(crate::bluetooth::State),
    BtDone(Result<String, String>),
    BtAgent(crate::bluetooth::AgentEvent),
    StoreFound(String, Result<Vec<crate::store::Found>, String>),
    StoreInstalled(std::collections::BTreeSet<String>),
    Store(crate::store::Event),
}

pub struct Shell {
    core: Core,
    layout: Layout,
    installed: HashMap<String, App>,
    /// Center → Apps: what each app may use, and the app being looked at
    denied: crate::sandbox::Choices,
    perm_app: Option<String>,
    /// Center → Screen: when the screen turns off and locks by itself
    idle: crate::idle::Times,
    /// Center → Screen: size, resolution, night light; and the screens
    display: crate::display::Settings,
    screens: Vec<crate::display::Screen>,
    /// a new resolution waits to be kept (Enter): otherwise the old one
    /// comes back at this moment
    /// a new resolution not kept yet: the saved mode before it, the mode
    /// the screen really had, and when it was changed
    display_revert: Option<(Option<(u16, u16, u32)>, Option<(u16, u16, u32)>, Instant)>,
    /// Bluetooth: a number to compare with the device, waiting for yes or no
    bt_confirm: Option<String>,
    /// Center → Bluetooth
    bt: crate::bluetooth::State,
    bt_busy: bool,
    bt_note: Option<String>,
    /// looking for devices until then (the list is refreshed meanwhile)
    bt_scan_until: Option<Instant>,
    /// Center → Get apps: what is installed, and the job under way
    store_installed: std::collections::BTreeSet<String>,
    store_job: Option<(crate::store::Job, String, u64, u64)>,
    store_note: Option<String>,
    /// Get apps: what was typed, and what the whole package set had for it
    store_search: String,
    store_found: Vec<crate::store::Found>,
    /// a search of every package is running (for this text)
    store_looking: Option<String>,
    /// told that this package is not free software: Enter again installs it
    store_unfree_ok: Option<String>,
    /// All apps: what was typed to find one
    search: String,
    /// notifications wait in the menu without popping up
    dnd: bool,
    pinned: Vec<String>,
    /// the update progress last read from its file
    seen_progress: Option<crate::progress::Progress>,
    /// how many apps Home shows (0: all of them)
    home_count: usize,
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
    /// All apps: a family of apps opened to choose one of them
    all_group: Option<String>,
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
    /// how much Chlorum's screens are zoomed (see ui::zoom_for)
    zoom: f32,
    /// keyboard layouts (niri's names) and the one in use
    kb_layouts: Vec<String>,
    kb_now: usize,
    /// a window that did not close when asked: X again forces it
    not_closing: Option<u64>,
    /// sound outputs and inputs, and the microphone
    audio: system::Devices,
    sound_note: Option<String>,
    wifi_on: Option<bool>,
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
            display: crate::display::load(),
            screens: vec![],
            display_revert: None,
            bt_confirm: None,
            bt: crate::bluetooth::State::default(),
            bt_busy: false,
            bt_note: None,
            bt_scan_until: None,
            store_installed: Default::default(),
            store_job: None,
            store_note: None,
            store_search: String::new(),
            store_found: vec![],
            store_looking: None,
            store_unfree_ok: None,
            search: String::new(),
            dnd: dnd_path().exists(),
            pinned: {
                add_gallery_once();
                apps::id_list("home-apps", apps::DEFAULT_PINNED)
            },
            home_count: load_home_count(),
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
            all_group: None,
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
            seen_progress: None,
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
            zoom: crate::ui::screen_zoom(),
            kb_layouts: vec![],
            kb_now: 0,
            not_closing: None,
            audio: system::Devices::default(),
            sound_note: None,
            wifi_on: None,
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
            Task::perform(system::devices(), |d| cosmic::Action::App(Message::Devices(d))),
            Task::perform(system::network(), |s| cosmic::Action::App(Message::Network(s))),
            Task::perform(system::wifi_on(), |on| cosmic::Action::App(Message::WifiOn(on))),
            // the screen as it was chosen in Center (scale, resolution)
            shell.apply_display(),
            // a minute after start, look for a new version (then every 6 hours)
            Task::perform(tokio::time::sleep(Duration::from_secs(60)), |_| cosmic::Action::App(Message::Command("check-update".into()))),
        ]);
        (shell, t)
    }

    fn subscription(&self) -> Subscription<Message> {
        let mut subs = vec![
            Subscription::run(niri::events).map(Message::Niri),
            Subscription::run(commands),
            Subscription::run(notify::events).map(Message::Note),
            Subscription::run(crate::bluetooth::agent).map(Message::BtAgent),
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
                // looking for Bluetooth devices: show new ones as they come
                if let Some(until) = self.bt_scan_until {
                    if Instant::now() > until {
                        self.bt_scan_until = None;
                        return Task::perform(
                            async {
                                let _ = crate::bluetooth::scan(false).await;
                                crate::bluetooth::state().await
                            },
                            |st| cosmic::Action::App(Message::Bt(st)),
                        );
                    }
                    if Local::now().timestamp_subsec_millis() < 250 {
                        return Task::perform(crate::bluetooth::state(), |st| cosmic::Action::App(Message::Bt(st)));
                    }
                }
                // a new resolution nobody kept: back to the one before
                if let Some((saved, real, at)) = self.display_revert {
                    if at.elapsed() > Duration::from_secs(15) {
                        self.display_revert = None;
                        self.display.mode = saved;
                        let _ = crate::display::save(&self.display);
                        self.toast("The screen is back to the resolution before.");
                        // put back the mode the screen really had, saved or not
                        let mut s = self.display.clone();
                        s.mode = real.or(saved);
                        return Task::perform(
                            async move {
                                crate::display::apply(&s).await;
                                crate::display::screens().await
                            },
                            |v| cosmic::Action::App(Message::Screens(v)),
                        );
                    }
                }
                // an app is asking for permission: its window takes the keyboard
                if crate::ask::asking() && (self.home.is_some() || self.menu.is_some()) {
                    self.close_home_now_if_open();
                    self.close_menu();
                }
                // an update started here or by the daily timer: follow it
                if let Some(p) = system::update_progress() {
                    if self.seen_progress.as_ref() != Some(&p) {
                        self.seen_progress = Some(p.clone());
                        self.update = match p.phase.as_str() {
                            "done" => Update::Installed(p.version.clone()),
                            "failed" => Update::Failed(if p.message.is_empty() { "the update stopped (details: /run/chlorum-update/update.log)".into() } else { p.message.clone() }),
                            _ => Update::Installing(Some(p)),
                        };
                    }
                }
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
            Message::Said(t) => self.toast(&t),
            Message::NotClosing(id) => {
                self.not_closing = Some(id);
                self.toast("It is not responding. Press X again to force it closed (unsaved work is lost).");
            }
            Message::Devices(d) => {
                self.audio = d;
                // the volume shown is the new output's
                return Task::perform(system::sound(), |s| cosmic::Action::App(Message::Sound(s)));
            }
            Message::SoundNote(n) => self.sound_note = n,
            Message::Screens(v) => {
                self.screens = v;
                // the screen's size changed (the size of everything, a new
                // resolution): Chlorum's screens zoom to fill it again
                if let Some(sc) = self.screens.first() {
                    if let Some((_, h, _)) = sc.current {
                        let logical = h as f64 / sc.scale.max(0.1);
                        let z = crate::ui::zoom_for(logical as f32);
                        eprintln!("chlorum: zoom {} (screen {} logical px tall), was {}", z, logical, self.zoom);
                        if (z - self.zoom).abs() > 0.01 {
                            self.zoom = z;
                            // a fresh configure, so the surfaces take the new zoom now
                            return Task::batch([cosmic::command::set_scaling_factor(z), set_size(self.surface, None, None)]);
                        }
                    }
                }
            }
            Message::Bt(st) => {
                self.bt = st;
                let rows = self.rows(Section::Bluetooth);
                if Section::ALL[self.section] == Section::Bluetooth {
                    if let Some(r) = self.row {
                        self.row = Some(r.min(rows.saturating_sub(1)));
                    }
                }
            }
            Message::BtDone(r) => {
                self.bt_busy = false;
                self.bt_confirm = None;
                self.bt_note = Some(match r {
                    Ok(m) => m,
                    Err(e) => e,
                });
                return Task::perform(crate::bluetooth::state(), |st| cosmic::Action::App(Message::Bt(st)));
            }
            Message::StoreInstalled(v) => self.store_installed = v,
            Message::StoreFound(q, r) => {
                if self.store_looking.as_deref() == Some(q.as_str()) {
                    self.store_looking = None;
                }
                if q == self.store_search {
                    match r {
                        Ok(v) => {
                            self.store_note = Some(if v.is_empty() { format!("Nothing else is called “{q}”.") } else { format!("{} more found. They are not checked for Chlorum, but most work.", v.len()) });
                            self.store_found = v;
                        }
                        Err(e) => self.store_note = Some(e),
                    }
                }
            }
            Message::Store(e) => match e {
                crate::store::Event::Progress(d, x) => {
                    if let Some(j) = self.store_job.as_mut() {
                        j.2 = d;
                        j.3 = x;
                    }
                }
                crate::store::Event::Done(r) => {
                    let attr = self.store_job.take().map(|j| j.1).unwrap_or_default();
                    self.store_unfree_ok = None;
                    self.store_note = Some(match r {
                        Ok(m) => m,
                        Err(e) => match e.strip_prefix("UNFREE:") {
                            Some(m) => {
                                self.store_unfree_ok = Some(attr);
                                m.to_string()
                            }
                            None => e,
                        },
                    });
                    // new apps on Home and in the lists
                    self.installed = apps::installed();
                    return Task::perform(crate::store::installed(), |v| cosmic::Action::App(Message::StoreInstalled(v)));
                }
            },
            Message::BtAgent(e) => {
                use crate::bluetooth::AgentEvent;
                match e {
                    AgentEvent::Show(code) => self.bt_note = Some(format!("Type {code} on the device, then its Enter key.")),
                    AgentEvent::Confirm(code) => {
                        self.bt_note = None;
                        self.bt_confirm = Some(code);
                    }
                    AgentEvent::Done => self.bt_confirm = None,
                }
            }
            Message::Network(n) => self.network = n,
            Message::Update(u) => {
                let start = matches!(u, Update::Available(_)) && matches!(self.update, Update::Installing(_));
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
            Message::WifiSwitched(r, on, list, net) => {
                self.wifi_busy = false;
                self.wifi_on = on;
                self.wifi = if on == Some(true) { list } else { vec![] };
                self.network = net;
                self.wifi_note = Some(match r {
                    Ok(m) => m,
                    Err(e) => e,
                });
            }
            Message::WifiOn(on) => {
                self.wifi_on = on;
                if on == Some(false) {
                    self.wifi.clear();
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
        size_limits: iced::Limits::NONE.min_width(1.0).min_height(1.0).max_width(480.0 * 4.0).max_height(132.0 * 4.0),
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

/// The line that fills while an update downloads and installs.
fn progress_line<'a>(x: f32, width: f32, f: f32) -> Element<'a, Message> {
    let x = x.clamp(0.0, 1.0);
    widget::Row::new()
        .push(fill(space((width * x).max(1.0), 6.0), alpha(YELLOW, f)))
        .push(fill(space((width * (1.0 - x)).max(1.0), 6.0), alpha(PANEL_2, f)))
        .into()
}

/// Choices for how many apps Home shows; 0 is all of them.
const HOME_COUNTS: &[usize] = &[4, 6, 8, 12, 16, 24, 0];

fn load_home_count() -> usize {
    match std::fs::read_to_string(apps::config_dir().join("chlorum/home-count")).map(|s| s.trim().to_string()) {
        Ok(s) if s == "all" => 0,
        Ok(s) => s.parse().ok().filter(|n| HOME_COUNTS.contains(n)).unwrap_or(6),
        Err(_) => 6,
    }
}

fn save_home_count(n: usize) -> std::io::Result<()> {
    let dir = apps::config_dir().join("chlorum");
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("home-count"), if n == 0 { "all\n".to_string() } else { format!("{n}\n") })
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

/// A row of Center → Get apps.
#[derive(Clone)]
enum StoreRow {
    Update,
    Entry(&'static crate::store::Entry),
    SearchAll,
    Found(crate::store::Found),
}

impl Shell {
    /// Asks a window to close. If it is still there 4 seconds later, the
    /// app hangs (or asks something): X again ends it for good.
    fn close_window(&mut self, id: u64) -> Task<Message> {
        if self.not_closing == Some(id) {
            self.not_closing = None;
            return Task::perform(
                async move {
                    let mut said = "It could not be forced closed: it shares its process with other windows. Close those, or log out.";
                    if let Ok(niri_ipc::Response::Windows(ws)) = niri::send(niri_ipc::Request::Windows).await {
                        if let Some(pid) = ws.iter().find(|w| w.id == id).and_then(|w| w.pid) {
                            // a process that shows other windows too (the X11
                            // bridge, a terminal server) is not ended for one
                            let shared = ws.iter().filter(|w| w.pid == Some(pid)).count() > 1;
                            let comm = std::fs::read_to_string(format!("/proc/{pid}/comm")).unwrap_or_default();
                            let bridge = ["xwayland-satel", "Xwayland", "foot", "niri"].iter().any(|b| comm.trim().starts_with(b));
                            if !shared && !bridge && pid > 1 {
                                // the app's process (the sandbox ends with it)
                                let _ = tokio::process::Command::new("kill").args(["-KILL", &pid.to_string()]).status().await;
                                said = "Forced closed.";
                            }
                        }
                    }
                    said.to_string()
                },
                |said| cosmic::Action::App(Message::Said(said)),
            );
        }
        Task::perform(
            async move {
                let _ = niri::run(vec![Action::CloseWindow { id: Some(id) }]).await;
                tokio::time::sleep(Duration::from_secs(4)).await;
                match niri::send(niri_ipc::Request::Windows).await {
                    Ok(niri_ipc::Response::Windows(ws)) => ws.iter().any(|w| w.id == id),
                    _ => false,
                }
            },
            move |still| cosmic::Action::App(if still { Message::NotClosing(id) } else { Message::Tick }),
        )
    }

    // ================================================================ surfaces

    fn open_home(&mut self) -> Task<Message> {
        if crate::ask::asking() {
            return Task::none();
        }
        self.home_closing = false;
        self.page = Page::Main;
        self.act = None;
        self.sel = 0;
        self.search.clear();
        self.all_group = None;
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
        if crate::ask::asking() && self.menu.is_none() {
            return Task::none();
        }
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
            Event::KeyboardLayoutsChanged { keyboard_layouts } => {
                self.kb_layouts = keyboard_layouts.names;
                self.kb_now = keyboard_layouts.current_idx as usize;
                return Task::none();
            }
            Event::KeyboardLayoutSwitched { idx } => {
                self.kb_now = idx as usize;
                return Task::none();
            }
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
            "check-update" => {
                let again = Task::perform(tokio::time::sleep(Duration::from_secs(6 * 3600)), |_| {
                    cosmic::Action::App(Message::Command("check-update".into()))
                });
                if matches!(self.update, Update::Idle | Update::UpToDate(_) | Update::Failed(_)) {
                    self.update = Update::Checking;
                    return Task::batch([Task::perform(system::check_update(), |u| cosmic::Action::App(Message::Update(u))), again]);
                }
                return again;
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
        // the chosen apps first, then the rest by name, as many as asked for
        let mut list: Vec<App> = self.pinned.iter().filter_map(|id| self.installed.get(id).cloned()).collect();
        for a in self.all_apps() {
            if !list.iter().any(|b| b.id == a.id) {
                list.push(a);
            }
        }
        let limit = if self.home_count == 0 { usize::MAX } else { self.home_count };
        for a in list.into_iter().filter(|a| !running_apps.contains(&a.id)).take(limit) {
            out.push(Tile::App(a));
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
                self.search.clear();
                self.all_group = None;
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
                return self.close_window(id);
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
        // Bluetooth asks: does the device show the same number?
        if self.bt_confirm.is_some() && self.page == Page::Center {
            match named {
                Some(Named::Enter) => {
                    self.bt_confirm = None;
                    crate::bluetooth::answer(true);
                }
                Some(Named::Escape) => {
                    self.bt_confirm = None;
                    crate::bluetooth::answer(false);
                }
                _ => {}
            }
            return Task::none();
        }
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
            Page::AllApps => self.all_key(named, &ch),
            Page::Center => self.center_key(named, &ch),
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
            // on the row of what to do with the tile: shortcuts
            "s" if acts.contains(&HomeAct::Beside) => return self.do_act(tile, HomeAct::Beside),
            "x" if acts.contains(&HomeAct::Close) => return self.do_act(tile, HomeAct::Close),
            // otherwise typing looks for an app
            c if c.chars().count() == 1 && c.chars().all(|x| x.is_alphanumeric()) => {
                self.search = c.to_string();
                self.all_sel = 0;
                self.page = Page::AllApps;
            }
            _ => {}
        }
        Task::none()
    }

    /// Every app, by name; with something typed, the ones that match, best
    /// first: the name starts with it, then a word in it does, then it is
    /// somewhere in the name or the app's id.
    /// All apps: one tile for each app, and one for each family of apps
    /// (Enter on it shows its members); with something typed, every app
    /// that matches, families or not.
    fn all_apps(&self) -> Vec<App> {
        self.apps_in(self.all_group.as_deref())
    }

    fn apps_in(&self, group: Option<&str>) -> Vec<App> {
        let list = self.all_apps_flat();
        if !self.search.is_empty() {
            return list;
        }
        let mut groups: std::collections::BTreeMap<String, Vec<App>> = std::collections::BTreeMap::new();
        for a in &list {
            groups.entry(apps::family(a)).or_default().push(a.clone());
        }
        // apps of different programs with the same name (two "Terminal"s)
        // are one family too
        let mut by_name: HashMap<String, Vec<String>> = HashMap::new();
        for (fam, members) in &groups {
            if members.len() == 1 {
                by_name.entry(members[0].name.to_lowercase()).or_default().push(fam.clone());
            }
        }
        for (name, fams) in by_name {
            if fams.len() > 1 {
                let mut merged = vec![];
                for f in &fams {
                    merged.extend(groups.remove(f).unwrap_or_default());
                }
                groups.insert(format!("name:{name}"), merged);
            }
        }
        if let Some(g) = group {
            let mut members = groups.remove(g).unwrap_or_default();
            members.sort_by_key(|a| a.name.to_lowercase());
            return members;
        }
        let mut out: Vec<App> = groups
            .into_iter()
            .map(|(key, members)| {
                if members.len() == 1 {
                    return members.into_iter().next().unwrap();
                }
                // the family's own name: its shortest (LibreOffice, not LibreOffice Writer)
                let lead = members.iter().min_by_key(|a| (a.name.len(), a.name.clone())).cloned().unwrap();
                App { id: format!("group:{key}"), name: lead.name.clone(), exec: String::new(), wm_class: None, permissions: None, mime: vec![], ..lead }
            })
            .collect();
        out.sort_by_key(|a| a.name.to_lowercase());
        out
    }

    /// How many apps a family tile stands for.
    fn group_size(&self, id: &str) -> usize {
        let Some(key) = id.strip_prefix("group:") else { return 1 };
        let list = self.all_apps_flat();
        if let Some(name) = key.strip_prefix("name:") {
            list.iter().filter(|a| a.name.to_lowercase() == name).count()
        } else {
            list.iter().filter(|a| apps::family(a) == key).count()
        }
    }

    fn all_apps_flat(&self) -> Vec<App> {
        let q = self.search.to_lowercase();
        let mut v: Vec<(u8, App)> = self
            .installed
            .values()
            .filter(|a| !apps::HIDDEN.contains(&a.id.as_str()) && !apps::hidden_by_name(a))
            .filter_map(|a| {
                if q.is_empty() {
                    return Some((0, a.clone()));
                }
                let name = a.name.to_lowercase();
                let rank = if name.starts_with(&q) {
                    0
                } else if name.split(|c: char| !c.is_alphanumeric()).any(|w| w.starts_with(&q)) {
                    1
                } else if name.contains(&q) || a.id.to_lowercase().contains(&q) {
                    2
                } else {
                    return None;
                };
                Some((rank, a.clone()))
            })
            .collect();
        v.sort_by_key(|(r, a)| (*r, a.name.to_lowercase()));
        v.into_iter().map(|(_, a)| a).collect()
    }

    fn all_key(&mut self, named: Option<keyboard::key::Named>, ch: &str) -> Task<Message> {
        use keyboard::key::Named;
        const COLS: usize = 6;
        // typing narrows the list; Backspace takes back; Esc clears first
        if named.is_none() && ch.chars().count() == 1 && ch.chars().all(|c| c.is_alphanumeric() || c == ' ' || c == '-') {
            self.search.push_str(ch);
            self.all_sel = 0;
            self.all_group = None;
            return Task::none();
        }
        // out of a family: back on its tile
        if named == Some(Named::Escape) && self.search.is_empty() {
            if let Some(g) = self.all_group.take() {
                let id = format!("group:{g}");
                self.all_sel = self.all_apps().iter().position(|a| a.id == id).unwrap_or(0);
                return Task::none();
            }
        }
        if named.is_none() && ch == " " {
            self.search.push(' ');
            return Task::none();
        }
        if named == Some(Named::Backspace) {
            self.search.pop();
            self.all_sel = 0;
            return Task::none();
        }
        if named == Some(Named::Escape) && !self.search.is_empty() {
            self.search.clear();
            self.all_sel = 0;
            return Task::none();
        }
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
                    if let Some(key) = a.id.strip_prefix("group:") {
                        self.all_group = Some(key.to_string());
                        self.all_sel = 0;
                        return Task::none();
                    }
                    self.page = Page::Main;
                    self.all_group = None;
                    self.search.clear();
                    return self.do_act(Tile::App(a.clone()), HomeAct::Open);
                }
            }
            _ => {}
        }
        Task::none()
    }

    fn center_key(&mut self, named: Option<keyboard::key::Named>, ch: &str) -> Task<Message> {
        use keyboard::key::Named;
        let section = Section::ALL[self.section];
        // Get apps: typing looks for an app
        if section == Section::GetApps && self.store_job.is_none() {
            let typed = named.is_none() && ch.chars().count() == 1 && ch.chars().all(|c| c.is_alphanumeric() || c == ' ' || c == '-' || c == '+' || c == '.');
            if typed && !(ch == " " && self.store_search.is_empty()) {
                self.store_search.push_str(ch);
                return self.store_searched();
            }
            if named == Some(Named::Backspace) && !self.store_search.is_empty() {
                self.store_search.pop();
                return self.store_searched();
            }
            if named == Some(Named::Escape) && !self.store_search.is_empty() {
                self.store_search.clear();
                return self.store_searched();
            }
        }
        let rows = self.rows(section);
        let Some(row) = self.row else {
            match named {
                Some(Named::ArrowDown) => self.section = (self.section + 1).min(Section::ALL.len() - 1),
                Some(Named::ArrowUp) => self.section = self.section.saturating_sub(1),
                Some(Named::ArrowRight) | Some(Named::Enter) if rows > 0 => self.row = Some(0),
                Some(Named::Escape) => self.page = Page::Main,
                _ => {}
            }
            // opening Get apps shows what is installed
            if Section::ALL[self.section] == Section::GetApps && matches!(named, Some(Named::ArrowDown) | Some(Named::ArrowUp)) {
                return Task::perform(crate::store::installed(), |v| cosmic::Action::App(Message::StoreInstalled(v)));
            }
            // opening Sound shows the outputs and inputs
            if Section::ALL[self.section] == Section::Sound && matches!(named, Some(Named::ArrowDown) | Some(Named::ArrowUp)) {
                return Task::batch([
                    Task::perform(system::devices(), |d| cosmic::Action::App(Message::Devices(d))),
                    Task::perform(system::sound(), |s| cosmic::Action::App(Message::Sound(s))),
                ]);
            }
            // opening Bluetooth shows what is there
            if Section::ALL[self.section] == Section::Bluetooth && matches!(named, Some(Named::ArrowDown) | Some(Named::ArrowUp)) {
                return Task::perform(crate::bluetooth::state(), |st| cosmic::Action::App(Message::Bt(st)));
            }
            // opening Network looks for Wi-Fi right away
            if Section::ALL[self.section] == Section::Network && matches!(named, Some(Named::ArrowDown) | Some(Named::ArrowUp)) {
                let list = if self.wifi.is_empty() && !self.wifi_busy {
                    self.wifi_busy = true;
                    Task::perform(system::wifi_list(), |v| cosmic::Action::App(Message::WifiList(v)))
                } else {
                    Task::none()
                };
                return Task::batch([Task::perform(system::wifi_on(), |on| cosmic::Action::App(Message::WifiOn(on))), list]);
            }
            return Task::none();
        };
        let flat = matches!(section, Section::Network | Section::Keyboard | Section::Bluetooth | Section::GetApps) || (section == Section::Apps && self.perm_app.is_none());
        // Delete removes an installed app
        if section == Section::GetApps && named == Some(Named::Delete) {
            return self.store_key(row, 0, true);
        }
        // Delete forgets a paired Bluetooth device
        if section == Section::Bluetooth && named == Some(Named::Delete) && !self.bt_busy {
            if let Some(d) = row.checked_sub(2).and_then(|i| self.bt.devices.get(i)).filter(|d| d.paired).cloned() {
                self.bt_busy = true;
                self.bt_note = Some(format!("Forgetting {}…", d.name));
                return Task::perform(crate::bluetooth::forget(d.path), |r| cosmic::Action::App(Message::BtDone(r)));
            }
        }
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
            Some(Named::Enter) if section == Section::Screen && self.display_revert.is_some() => self.keep_resolution(),
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
                let outs = self.audio.outputs.len();
                let devs = |d| cosmic::Action::App(Message::Devices(d));
                match row {
                    0 if dir != 0 => {
                        let v = ((self.sound.volume + d * 0.05) * 20.0).round() / 20.0;
                        return Task::perform(system::set_volume(v), |s| cosmic::Action::App(Message::Sound(s)));
                    }
                    1 if dir == 0 => return Task::perform(system::toggle_mute(), |s| cosmic::Action::App(Message::Sound(s))),
                    2 if dir == 0 => {
                        let out = self.audio.outputs.iter().find(|o| o.default).map(|o| o.name.clone()).unwrap_or_else(|| "the speakers".into());
                        self.sound_note = Some(format!("Playing a chime on {out}…"));
                        return Task::perform(system::test_sound(), move |r| {
                            cosmic::Action::App(Message::SoundNote(Some(match r {
                                Ok(()) => format!("Played on {out}. Nothing heard? Choose another output below."),
                                Err(e) => format!("Could not play: {e}"),
                            })))
                        });
                    }
                    r if (3..3 + outs).contains(&r) && dir == 0 => {
                        let id = self.audio.outputs[r - 3].id;
                        self.sound_note = Some(format!("Sound now comes out of {}.", self.audio.outputs[r - 3].name));
                        return Task::perform(system::set_default_device(id), devs);
                    }
                    r if r == 3 + outs && dir != 0 => {
                        let v = ((self.audio.mic_volume + d * 0.05) * 20.0).round() / 20.0;
                        return Task::perform(system::set_mic_volume(v), devs);
                    }
                    r if r == 4 + outs && dir == 0 => return Task::perform(system::toggle_mic(), devs),
                    r if r >= 5 + outs && dir == 0 => {
                        if let Some(dev) = self.audio.inputs.get(r - 5 - outs) {
                            self.sound_note = Some(format!("Listening with {}.", dev.name));
                            return Task::perform(system::set_default_device(dev.id), devs);
                        }
                    }
                    _ => {}
                }
            }
            Section::Updates => {
                if dir == 0 {
                    return self.update_action();
                }
            }
            Section::Bluetooth => return self.bluetooth_key(row, dir),
            Section::GetApps => return self.store_key(row, dir, false),
            Section::Network => {
                if dir != 0 {
                    return Task::none();
                }
                // row 0: Wi-Fi on or off
                if row == 0 {
                    let Some(on) = self.wifi_on else { return Task::none() };
                    self.wifi_busy = true;
                    self.wifi_note = Some(if on { "Switching Wi-Fi off…".into() } else { "Switching Wi-Fi on…".into() });
                    return Task::perform(
                        async move {
                            let r = system::set_wifi(!on).await;
                            if !on {
                                // the radio needs a moment before it sees networks
                                tokio::time::sleep(Duration::from_secs(3)).await;
                            }
                            (r, system::wifi_on().await, system::wifi_list().await, system::network().await)
                        },
                        |(r, on, list, net)| {
                            cosmic::Action::App(Message::WifiSwitched(r.map(|()| if on == Some(true) { "Wi-Fi is on.".into() } else { "Wi-Fi is off.".into() }), on, list, net))
                        },
                    );
                }
                if self.wifi_on != Some(true) {
                    return Task::none();
                }
                if row == 1 {
                    self.wifi_busy = true;
                    self.wifi_note = None;
                    return Task::perform(system::wifi_list(), |v| cosmic::Action::App(Message::WifiList(v)));
                }
                if let Some(w) = self.wifi.get(row - 2).cloned() {
                    // the one it is on: Enter leaves it
                    if w.active {
                        self.wifi_busy = true;
                        self.wifi_note = Some(format!("Leaving {}…", w.ssid));
                        return Task::perform(
                            async move { (system::wifi_disconnect(w.ssid).await.map(|()| "Disconnected.".to_string()), system::wifi_on().await, system::wifi_list().await, system::network().await) },
                            |(r, on, list, net)| cosmic::Action::App(Message::WifiSwitched(r, on, list, net)),
                        );
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
            Section::Home => {
                if dir != 0 {
                    let i = HOME_COUNTS.iter().position(|c| *c == self.home_count).unwrap_or(0) as i32;
                    self.home_count = HOME_COUNTS[(i + dir).clamp(0, HOME_COUNTS.len() as i32 - 1) as usize];
                    if let Err(e) = save_home_count(self.home_count) {
                        self.toast(&format!("Could not save: {e}"));
                    }
                }
            }
            Section::Screen if row >= 2 => return self.screen_key(row, dir),
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
            Section::About | Section::Shortcuts => {}
        }
        Task::none()
    }

    /// Center → Get apps: row 0 brings every installed app up to date, then
    /// the catalog (Enter installs, Delete removes). One job at a time.
    /// What Get apps lists: updating, the catalogue's apps (those that
    /// match what was typed), then looking through every package and what
    /// that found.
    fn store_rows(&self) -> Vec<StoreRow> {
        let mut v = vec![StoreRow::Update];
        v.extend(crate::store::find(&self.store_search).into_iter().map(StoreRow::Entry));
        if self.store_search.trim().chars().count() >= 2 {
            v.push(StoreRow::SearchAll);
            v.extend(self.store_found.iter().cloned().map(StoreRow::Found));
        }
        v
    }

    /// After a letter typed or taken back: the first match is chosen.
    fn store_searched(&mut self) -> Task<Message> {
        self.store_found.clear();
        self.store_note = None;
        self.row = Some(if self.store_search.is_empty() { 0 } else { 1 });
        Task::none()
    }

    fn store_key(&mut self, row: usize, dir: i32, remove: bool) -> Task<Message> {
        use crate::store::Job;
        if dir != 0 || self.store_job.is_some() {
            return Task::none();
        }
        let rows = self.store_rows();
        let Some(r) = rows.get(row) else { return Task::none() };
        let (job, attr, name) = match r {
            StoreRow::Update if !remove => (Job::Update, String::new(), String::new()),
            StoreRow::Update => return Task::none(),
            StoreRow::SearchAll => {
                if remove || self.store_looking.is_some() {
                    return Task::none();
                }
                let q = self.store_search.trim().to_string();
                self.store_looking = Some(q.clone());
                self.store_note = Some(format!("Looking through every package for “{q}”… The first time takes a minute or two."));
                return Task::perform(crate::store::search_all(q.clone()), move |r| cosmic::Action::App(Message::StoreFound(q.clone(), r)));
            }
            StoreRow::Entry(e) => (Job::Install, e.attr.to_string(), e.name.to_string()),
            StoreRow::Found(f) => (Job::Install, f.attr.clone(), f.name.clone()),
        };
        if job == Job::Install {
            let have = self.store_installed.contains(&attr);
            match (remove, have) {
                (true, true) => return self.store_start(Job::Remove, attr),
                (false, false) => {}
                (false, true) => {
                    self.store_note = Some(format!("{name} is installed already. Delete removes it."));
                    return Task::none();
                }
                (true, false) => return Task::none(),
            }
        }
        // the second Enter on a package said to be not free software
        let job = if job == Job::Install && self.store_unfree_ok.as_deref() == Some(attr.as_str()) { Job::InstallUnfree } else { job };
        self.store_start(job, attr)
    }

    fn store_start(&mut self, job: crate::store::Job, attr: String) -> Task<Message> {
        self.store_note = None;
        self.store_job = Some((job, attr.clone(), 0, 0));
        Task::run(crate::store::run(job, attr), |e| cosmic::Action::App(Message::Store(e)))
    }

    fn store_view(&self, f: f32) -> Element<'_, Message> {
        use crate::store::Job;
        let mut col = widget::Column::new().spacing(8);
        col = col.push(text("Installed for you alone, ready-built for this Chlorum. Each app asks for what it may use when it first starts.", SANS, 19.0, alpha(TEXT_2, f))).push(space(1.0, 8.0));
        if let Some((job, attr, done, expected)) = &self.store_job {
            let name = crate::store::entry(attr).map(|e| e.name).unwrap_or("Your apps");
            let what = match job {
                Job::Install | Job::InstallUnfree => format!("Installing {name}"),
                Job::Remove => format!("Removing {name}"),
                Job::Update => "Updating your apps".to_string(),
            };
            let pct = if *expected > 0 { (*done * 100 / *expected).min(100) } else { 0 };
            col = col
                .push(text(if *expected > 0 { format!("{what} · {pct}%") } else { format!("{what}…") }, SANS_BOLD, 20.0, alpha(YELLOW, f)))
                .push(progress_line(if *expected > 0 { *done as f32 / *expected as f32 } else { 0.02 }, 900.0, f))
                .push(space(1.0, 6.0));
        }
        if let Some(n) = &self.store_note {
            col = col.push(text(n.clone(), SANS_BOLD, 19.0, alpha(YELLOW, f))).push(space(1.0, 6.0));
        }
        let title = if self.store_search.is_empty() { "TYPE TO FIND AN APP".to_string() } else { format!("{}▏", self.store_search) };
        col = col.push(text(title, if self.store_search.is_empty() { MONO } else { SANS_BOLD }, if self.store_search.is_empty() { 15.0 } else { 34.0 }, alpha(if self.store_search.is_empty() { TEXT_2 } else { YELLOW }, f)));
        let rows = self.store_rows();
        let range = self.window(0, rows.len(), 12);
        for (i, r) in rows.iter().enumerate().filter(|(i, _)| range.contains(i)) {
            let (label, state) = match r {
                StoreRow::Update => ("Update the apps installed here".to_string(), format!("{} INSTALLED", self.store_installed.len())),
                StoreRow::Entry(e) => (
                    format!("{}  ·  {}", e.name, e.about),
                    if self.store_installed.contains(e.attr) {
                        "INSTALLED · DEL REMOVES".to_string()
                    } else if e.unfree {
                        format!("{} · NOT FREE SOFTWARE", e.kind.to_uppercase())
                    } else {
                        e.kind.to_uppercase()
                    },
                ),
                StoreRow::SearchAll => (
                    if self.store_looking.is_some() { "Looking through every package…".to_string() } else { format!("Look through every package for “{}”", self.store_search.trim()) },
                    "ENTER · ABOUT 100 000".to_string(),
                ),
                StoreRow::Found(x) => (
                    if x.about.is_empty() { x.name.clone() } else { format!("{}  ·  {}", x.name, x.about) },
                    if self.store_installed.contains(&x.attr) { "INSTALLED · DEL REMOVES".to_string() } else { x.attr.to_uppercase() },
                ),
            };
            col = col.push(self.list_row(i, label, state, f));
        }
        let n = rows.iter().filter(|r| matches!(r, StoreRow::Entry(_) | StoreRow::Found(_))).count();
        if n == 0 && !self.store_search.is_empty() {
            col = col.push(text("None in the catalogue. Enter on the row above looks through every package.", SANS, 18.0, alpha(TEXT_2, f)));
        }
        col = col.push(text(format!("{} apps", n), MONO, 14.0, alpha(TEXT_2, f)));
        col.into()
    }

    /// Center → Bluetooth: row 0 switches it on or off, row 1 looks for
    /// devices, then the devices (Enter connects, pairing first when
    /// needed, or disconnects).
    fn bluetooth_key(&mut self, row: usize, dir: i32) -> Task<Message> {
        if dir != 0 || self.bt_busy {
            return Task::none();
        }
        self.bt_note = None;
        let done = |r| cosmic::Action::App(Message::BtDone(r));
        match row {
            0 => {
                let on = !self.bt.powered;
                self.bt_busy = true;
                Task::perform(
                    async move { crate::bluetooth::set_powered(on).await.map(|()| if on { "Bluetooth is on.".to_string() } else { "Bluetooth is off.".to_string() }) },
                    done,
                )
            }
            1 => {
                self.bt_scan_until = Some(Instant::now() + Duration::from_secs(30));
                self.bt_note = Some("Looking for devices for 30 seconds. Put yours in pairing mode.".into());
                Task::perform(crate::bluetooth::scan(true), |r| cosmic::Action::App(Message::BtDone(r.map(|()| "Looking for devices…".to_string()))))
            }
            r => {
                let Some(d) = self.bt.devices.get(r - 2).cloned() else { return Task::none() };
                self.bt_busy = true;
                if d.connected {
                    self.bt_note = Some(format!("Disconnecting {}…", d.name));
                    Task::perform(crate::bluetooth::disconnect(d.path), done)
                } else {
                    self.bt_note = Some(if d.paired { format!("Connecting {}…", d.name) } else { format!("Pairing with {}…", d.name) });
                    Task::perform(crate::bluetooth::connect(d.path), done)
                }
            }
        }
    }

    /// Enter while a new resolution waits: it stays, and is saved.
    fn keep_resolution(&mut self) {
        if self.display_revert.take().is_some() {
            match crate::display::save(&self.display) {
                Ok(()) => self.toast("Resolution kept."),
                Err(e) => self.toast(&format!("Could not save: {e}")),
            }
        }
    }

    /// Puts the chosen scale and resolution on the screens, then looks at
    /// them again.
    fn apply_display(&self) -> Task<Message> {
        let s = self.display.clone();
        Task::perform(
            async move {
                crate::display::apply(&s).await;
                crate::display::screens().await
            },
            |v| cosmic::Action::App(Message::Screens(v)),
        )
    }

    /// Center → Screen, below the two idle rows: scale, resolution, refresh
    /// rate (when there is a screen to ask), night light, warmth.
    fn screen_key(&mut self, row: usize, dir: i32) -> Task<Message> {
        use crate::display::{Night, SCALES, WARMTH};
        let step = |len: usize, i: usize, dir: i32| (i as i32 + dir).clamp(0, len as i32 - 1) as usize;
        // without a screen to ask, the rows start at the night light
        let row = match self.screens.first() {
            None => row + 3,
            // a screen whose mode is not known: no resolution rows
            Some(sc) if row >= 3 && self.display.mode.filter(|m| sc.modes.contains(m)).or(sc.current).is_none() => row + 2,
            Some(_) => row,
        };
        // Enter keeps a new resolution
        if dir == 0 {
            self.keep_resolution();
            return Task::none();
        }
        let sc = self.screens.first().cloned();
        match row {
            2 => {
                let now = self.display.scale.or(sc.as_ref().map(|s| s.scale)).unwrap_or(1.0);
                let i = SCALES.iter().position(|x| (x - now).abs() < 0.01).unwrap_or_else(|| SCALES.iter().position(|x| *x > now).unwrap_or(0));
                self.display.scale = Some(SCALES[step(SCALES.len(), i, dir)]);
            }
            3 | 4 => {
                let Some(sc) = sc else { return Task::none() };
                let now = self.display.mode.filter(|m| sc.modes.contains(m)).or(sc.current);
                let Some((w, h, r)) = now else { return Task::none() };
                let new = if row == 3 {
                    let sizes = sc.sizes();
                    let i = sizes.iter().position(|x| *x == (w, h)).unwrap_or(0);
                    let size = sizes[step(sizes.len(), i, dir)];
                    // the same rate if it has it, otherwise its fastest
                    let rates = sc.rates(size);
                    let rate = if rates.contains(&r) { r } else { rates.first().copied().unwrap_or(r) };
                    (size.0, size.1, rate)
                } else {
                    let rates = sc.rates((w, h));
                    // ← slower, → faster: the list is fastest first
                    let i = rates.iter().position(|x| *x == r).unwrap_or(0);
                    (w, h, rates[step(rates.len(), i, -dir)])
                };
                if Some(new) == now {
                    return Task::none();
                }
                if self.display_revert.is_none() {
                    self.display_revert = Some((self.display.mode, sc.current, Instant::now()));
                } else if let Some((_, _, at)) = self.display_revert.as_mut() {
                    *at = Instant::now();
                }
                self.display.mode = Some(new);
                self.toast("Press Enter to keep this resolution. Otherwise it goes back in 15 seconds.");
            }
            5 => {
                let i = Night::ALL.iter().position(|n| *n == self.display.night).unwrap_or(0);
                self.display.night = Night::ALL[step(Night::ALL.len(), i, dir)];
            }
            6 => {
                let i = WARMTH.iter().position(|w| w.0 == self.display.warmth).unwrap_or(1);
                self.display.warmth = WARMTH[step(WARMTH.len(), i, dir)].0;
            }
            _ => return Task::none(),
        }
        // a resolution not kept yet is not saved: a screen that stays black
        // must not come back like that at the next login
        let mut on_disk = self.display.clone();
        if let Some((saved, _, _)) = self.display_revert {
            on_disk.mode = saved;
        }
        if let Err(e) = crate::display::save(&on_disk) {
            self.toast(&format!("Could not save: {e}"));
        }
        if row >= 5 {
            crate::display::restart_nightlight();
            return Task::none();
        }
        self.apply_display()
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
            Section::Network => match self.wifi_on {
                None => 0,
                Some(false) => 1,
                Some(true) => 2 + self.wifi.len(),
            },
            Section::Bluetooth => if !self.bt.present { 0 } else if self.bt.powered { 2 + self.bt.devices.len() } else { 1 },
            Section::Sound => {
                let mic = if self.audio.inputs.is_empty() { 0 } else { 2 + self.audio.inputs.len() };
                if self.sound.present || !self.audio.outputs.is_empty() { 3 + self.audio.outputs.len() + mic } else { 0 }
            }
            Section::Keyboard => crate::setupd::LAYOUTS.len(),
            Section::Screen => match self.screens.first() {
                None => 4,
                Some(sc) if self.display.mode.filter(|m| sc.modes.contains(m)).or(sc.current).is_none() => 5,
                Some(_) => 7,
            },
            Section::Home => 1,
            Section::Apps => if self.perm_app.is_some() { crate::sandbox::Perm::ALL.len() } else { self.app_list().len() },
            Section::GetApps => self.store_rows().len(),
            Section::Updates => 1,
            Section::About | Section::Shortcuts => 0,
        }
    }

    fn update_action(&mut self) -> Task<Message> {
        match self.update {
            Update::Checking | Update::Installing(_) => Task::none(),
            Update::Available(_) => {
                self.update = Update::Installing(None);
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
        if self.kb_layouts.len() > 1 {
            if let Some(name) = self.kb_layouts.get(self.kb_now) {
                chips = chips.push(chip(format!("{} · ALT+SHIFT", layout_short(name)), false));
            }
        }
        chips = chips.push(chip(format!("NET · {}", self.network.to_uppercase()), false));
        if self.sound.present {
            let v = if self.sound.muted { "MUTED".to_string() } else { format!("VOL {:.0}", self.sound.volume * 100.0) };
            chips = chips.push(chip(v, false));
        }
        if matches!(self.update, Update::Available(_) | Update::Installed(_)) {
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
        // a long row slides along with the choice
        const SHOWN: usize = 14;
        let start = sel.saturating_sub(SHOWN / 2).min(tiles.len().saturating_sub(SHOWN));
        for (i, t) in tiles.iter().enumerate().skip(start).take(SHOWN) {
            ribbon = ribbon.push(self.tile_view(i, t, f));
        }
        if tiles.len() > SHOWN {
            ribbon = ribbon.push(text(format!("{} / {}", sel + 1, tiles.len()), MONO, 15.0, alpha(TEXT_2, f)));
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
        h.push(("TYPE", "Find an app", false));
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
        // the rows around the chosen app, so it is always on screen
        const ROWS: usize = 4;
        let rows = list.len().div_ceil(6);
        let first = (self.all_sel / 6).saturating_sub(ROWS / 2).min(rows.saturating_sub(ROWS));
        for (r, chunk) in list.chunks(6).enumerate().skip(first).take(ROWS) {
            let mut row = widget::Row::new().spacing(24);
            for (c, a) in chunk.iter().enumerate() {
                let i = r * 6 + c;
                let on = i == self.all_sel;
                let mut inner = widget::Column::new()
                    .push(widget::icon::from_name(a.icon.clone()).size(72))
                    .push(text(a.name.clone(), SANS_BOLD, 19.0, alpha(if on { BLACK } else { TEXT }, f)))
                    .spacing(16)
                    .align_x(Alignment::Center);
                if a.id.starts_with("group:") {
                    inner = inner.push(text(format!("{} APPS · ENTER", self.group_size(&a.id)), MONO, 13.0, alpha(if on { BLACK } else { TEXT_2 }, f)));
                }
                let card = fill(
                    inner,
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
        if list.is_empty() {
            grid = grid.push(text("No app is called like that. Backspace takes a letter back; Get apps in Center has more.", SANS, 22.0, alpha(TEXT_2, f)));
        }
        let group_name = self.all_group.as_ref().and_then(|g| {
            let id = format!("group:{g}");
            self.apps_in(None).into_iter().find(|a| a.id == id).map(|a| a.name)
        });
        let title = if !self.search.is_empty() {
            format!("{}▏", self.search)
        } else if let Some(n) = &group_name {
            n.clone()
        } else {
            "All apps".to_string()
        };
        let more = if rows > ROWS { format!("{} of {} apps", (self.all_sel + 1).min(list.len()), list.len()) } else { String::new() };
        widget::Column::new()
            .push(text(title, SANS_BOLD, 52.0, alpha(if self.search.is_empty() { TEXT } else { YELLOW }, f)))
            .push(text(if self.search.is_empty() { "Type to find an app".to_string() } else { more.clone() }, MONO, 16.0, alpha(TEXT_2, f)))
            .push(space(1.0, 24.0))
            .push(widget::container(grid).height(Length::Fill))
            .push(text(if self.search.is_empty() { more } else { String::new() }, MONO, 14.0, alpha(TEXT_2, f)))
            .push(space(1.0, 12.0))
            .push(hints(&[("TYPE", "Find", false), ("← ↑ ↓ →", "Move", false), ("ENTER", "Open", true), ("ESC", if !self.search.is_empty() { "Clear" } else if self.all_group.is_some() { "All apps" } else { "Back" }, false)], f))
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
                fill(text(s.label(), SANS_BOLD, 24.0, alpha(fg, f)), alpha(bg, f))
                    .padding([11, 28])
                    .width(Length::Fill),
            );
        }
        let content = match Section::ALL[self.section] {
            Section::Wallpaper => self.wallpaper_view(f),
            Section::Network => self.network_view(f),
            Section::Bluetooth => self.bluetooth_view(f),
            Section::Sound => self.sound_view(f),
            Section::Keyboard => self.keyboard_view(f),
            Section::Screen => self.screen_view(f),
            Section::Home => self.home_settings_view(f),
            Section::Apps => self.apps_view(f),
            Section::GetApps => self.store_view(f),
            Section::Updates => self.updates_view(f),
            Section::About => self.about_view(f),
            Section::Shortcuts => self.shortcuts_view(f),
        };
        let sec = Section::ALL[self.section];
        let flat = matches!(sec, Section::Network | Section::Keyboard);
        let h: &[(&str, &str, bool)] = if self.wifi_entry.is_some() {
            &[("ENTER", "Connect", true), ("ESC", "Cancel", false)]
        } else if sec == Section::Apps && self.row.is_some() && self.perm_app.is_none() {
            &[("↑ ↓", "App", false), ("ENTER", "What it may use", true), ("ESC", "Sections", false)]
        } else if sec == Section::Apps && self.row.is_some() {
            &[("↑ ↓", "Choose", false), ("← →", "Off · On", true), ("ENTER", "Switch", false), ("ESC", "Apps", false)]
        } else if sec == Section::GetApps && self.row.is_some() {
            &[("TYPE", "Find", false), ("↑ ↓", "Choose", false), ("ENTER", "Install", true), ("DEL", "Remove", false), ("ESC", "Clear / Sections", false)]
        } else if sec == Section::Bluetooth && self.row.is_some() {
            &[("↑ ↓", "Choose", false), ("ENTER", "Connect · Disconnect", true), ("DEL", "Forget", false), ("ESC", "Sections", false)]
        } else if sec == Section::Screen && self.display_revert.is_some() {
            &[("ENTER", "Keep this resolution", true), ("← →", "Change", false), ("ESC", "Sections", false)]
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
            .push(
                widget::Row::new()
                    .push(sections)
                    .push(space(70.0, 1.0))
                    // as wide as the screen allows, but lines stay readable
                    .push(widget::container(content).width(Length::Fill).max_width(1600.0))
                    .height(Length::Fill),
            )
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
            .width(Length::Fill);
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
            .push(text(wallpaper_progress(), MONO, 15.0, alpha(TEXT_2, f)))
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
        .width(Length::Fill)
        .into()
    }

    /// Which rows of a long list to show: a window around the chosen one,
    /// so the selection never goes off screen.
    fn window(&self, first: usize, len: usize, shown: usize) -> std::ops::Range<usize> {
        let sel = self.row.unwrap_or(0).saturating_sub(first);
        let start = sel.saturating_sub(shown / 2).min(len.saturating_sub(shown));
        start..(start + shown).min(len)
    }

    fn bluetooth_view(&self, f: f32) -> Element<'_, Message> {
        let mut col = widget::Column::new().spacing(8);
        if !self.bt.present {
            return col.push(text("There is no Bluetooth on this computer (or it is switched off in its settings).", SANS, 20.0, alpha(TEXT_2, f))).into();
        }
        col = col.push(self.list_row(0, if self.bt.powered { "Bluetooth is on".into() } else { "Bluetooth is off".into() }, if self.bt.powered { "ENTER TURNS OFF".into() } else { "ENTER TURNS ON".into() }, f));
        if self.bt.powered {
            let looking = self.bt_scan_until.is_some();
            col = col.push(self.list_row(1, if looking { "Looking for devices…".into() } else { "Look for devices to pair".into() }, "ENTER".into(), f)).push(space(1.0, 10.0));
            let n = self.bt.devices.len();
            let range = self.window(2, n, 8);
            for i in range.clone() {
                let d = &self.bt.devices[i];
                let state = if d.connected {
                    format!("CONNECTED{}", d.battery.map(|b| format!(" · {b}%")).unwrap_or_default())
                } else if d.paired {
                    "PAIRED · ENTER CONNECTS".into()
                } else {
                    "NEARBY · ENTER PAIRS".into()
                };
                col = col.push(self.list_row(i + 2, format!("{}  ·  {}", d.name, d.kind()), state, f));
            }
            if n > range.len() {
                col = col.push(text(format!("{} of {}", self.row.unwrap_or(2).saturating_sub(1).clamp(1, n), n), MONO, 14.0, alpha(TEXT_2, f)));
            }
            if n == 0 && !looking {
                col = col.push(text("No devices yet. Look for devices, with yours in pairing mode.", SANS, 18.0, alpha(TEXT_2, f)));
            }
        }
        if self.bt_busy {
            col = col.push(crate::atom::Atom::view(64.0, self.started_at));
        }
        if let Some(n) = &self.bt_note {
            col = col.push(text(n.clone(), SANS_BOLD, 20.0, alpha(YELLOW, f)));
        }
        if let Some(code) = &self.bt_confirm {
            col = col
                .push(space(1.0, 10.0))
                .push(text("Does the device show this number?", SANS_BOLD, 24.0, alpha(TEXT, f)))
                .push(text(code.clone(), MONO_BOLD, 48.0, alpha(YELLOW, f)))
                .push(hints(&[("ENTER", "Yes, the same", true), ("ESC", "No", false)], f));
        }
        col.into()
    }

    fn network_view(&self, f: f32) -> Element<'_, Message> {
        let mut col = widget::Column::new().spacing(8);
        col = col
            .push(text(format!("Now: {}", if self.network.is_empty() { "Offline" } else { &self.network }), SANS_BOLD, 28.0, alpha(TEXT, f)))
            .push(space(1.0, 14.0));
        match self.wifi_on {
            None => col = col.push(text("There is no Wi-Fi on this computer. A cable works by itself.", SANS, 20.0, alpha(TEXT_2, f))),
            Some(on) => {
                col = col.push(self.list_row(0, "Wi-Fi".into(), if on { "ON · ENTER SWITCHES OFF".into() } else { "OFF · ENTER SWITCHES ON".into() }, f));
            }
        }
        if self.wifi_on == Some(true) {
            let search = if self.wifi_busy && self.wifi_entry.is_none() { "Searching…" } else { "Search for Wi-Fi again" };
            col = col.push(self.list_row(1, search.into(), "ENTER".into(), f));
            let range = self.window(2, self.wifi.len(), 9);
            for (i, w) in self.wifi.iter().enumerate().filter(|(i, _)| range.contains(i)) {
                let detail = if w.active {
                    "CONNECTED · ENTER LEAVES".to_string()
                } else {
                    format!("{}{}%", if w.secure { "LOCKED · " } else { "" }, w.signal)
                };
                col = col.push(self.list_row(i + 2, w.ssid.clone(), detail, f));
            }
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

    fn home_settings_view(&self, f: f32) -> Element<'_, Message> {
        let label = |c: usize| if c == 0 { "All".to_string() } else { c.to_string() };
        let mut r = widget::Row::new().spacing(10);
        for c in HOME_COUNTS {
            let sel = *c == self.home_count;
            r = r.push(
                fill(text(label(*c), SANS_BOLD, 18.0, alpha(if sel { BLACK } else { TEXT }, f)), alpha(if sel { YELLOW } else { PANEL_2 }, f))
                    .padding([12, 20]),
            );
        }
        widget::Column::new()
            .push(text("The apps in the row on Home. Your own choice comes first, then the others by name; open apps are always there.", SANS, 20.0, alpha(TEXT_2, f)))
            .push(self.row_box(0, "Apps on Home", label(self.home_count), r.into(), f))
            .spacing(26)
            .into()
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
        use crate::display::{Night, SCALES, WARMTH, rate_label, scale_label, size_label};
        // one chip per choice, the chosen one yellow
        let chips = |items: Vec<String>, chosen: Option<usize>, on: bool| -> Element<'_, Message> {
            let mut r = widget::Row::new().spacing(10);
            for (i, t) in items.into_iter().enumerate() {
                let sel = Some(i) == chosen;
                r = r.push(
                    fill(text(t, SANS_BOLD, 16.0, alpha(if sel { BLACK } else if on { TEXT } else { TEXT_2 }, f)), alpha(if sel { YELLOW } else { PANEL_2 }, f))
                        .padding([12, 14]),
                );
            }
            r.into()
        };
        let mut col = widget::Column::new()
            .push(text("When nobody uses the computer. A video or a call keeps it awake.", SANS, 20.0, alpha(TEXT_2, f)))
            .push(self.row_box(0, "Turn the screen off after", label(self.idle.screen_off), choice(SCREEN_OFF, self.idle.screen_off, self.row == Some(0)), f))
            .push(self.row_box(1, "Lock after", label(self.idle.lock), choice(LOCK, self.idle.lock, self.row == Some(1)), f))
            .spacing(26);
        let mut next = 2;
        if let Some(sc) = self.screens.first() {
            let scale = self.display.scale.unwrap_or(sc.scale);
            let si = SCALES.iter().position(|x| (x - scale).abs() < 0.01);
            col = col.push(self.row_box(2, "Size of apps", scale_label(scale), chips(SCALES.iter().map(|x| scale_label(*x)).collect(), si, self.row == Some(2)), f));
            if let Some((w, h, r)) = self.display.mode.filter(|m| sc.modes.contains(m)).or(sc.current) {
                let sizes = sc.sizes();
                let i = sizes.iter().position(|x| *x == (w, h)).unwrap_or(0);
                // a window of sizes around the chosen one: some screens offer many
                let from = i.saturating_sub(2).min(sizes.len().saturating_sub(5));
                let shown: Vec<String> = sizes.iter().skip(from).take(5).map(|s| size_label(s.0, s.1)).collect();
                let keep = if self.display_revert.is_some() { "  ·  ENTER KEEPS IT" } else { "" };
                col = col.push(self.row_box(3, "Resolution", format!("{}{keep}", size_label(w, h)), chips(shown, Some(i - from), self.row == Some(3)), f));
                let rates = sc.rates((w, h));
                let ri = rates.iter().position(|x| *x == r);
                col = col.push(self.row_box(4, "Refresh rate", rate_label(r), chips(rates.iter().map(|x| rate_label(*x)).collect(), ri, self.row == Some(4)), f));
            }
            next = 5;
        }
        let ni = Night::ALL.iter().position(|n| *n == self.display.night);
        col = col.push(self.row_box(next, "Night light", self.display.night.label().to_string(), chips(Night::ALL.iter().map(|n| n.label().to_string()).collect(), ni, self.row == Some(next)), f));
        let wi = WARMTH.iter().position(|w| w.0 == self.display.warmth);
        col = col.push(self.row_box(next + 1, "Warmth at night", format!("{} K", self.display.warmth), chips(WARMTH.iter().map(|w| w.1.to_string()).collect(), wi, self.row == Some(next + 1)), f));
        col.into()
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
                                .push(text(if on { "Allowed." } else if !crate::sandbox::answered(&self.denied, app, *p) { "Not allowed: it has not asked for this." } else { p.off_means() }, SANS, 16.0, alpha(if sel { BLACK } else { TEXT_2 }, f)))
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
            .push(text("Apps may use nothing until you allow it. Choose one to see what it may use: internet, camera, sound, your files.", SANS, 20.0, alpha(TEXT_2, f)))
            .push(space(1.0, 10.0));
        let list = self.app_list();
        // the chosen app stays in view: show a window of the list around it
        const SHOWN: usize = 9;
        let sel = self.row.unwrap_or(0);
        let start = sel.saturating_sub(SHOWN / 2).min(list.len().saturating_sub(SHOWN));
        for (i, a) in list.iter().enumerate().skip(start).take(SHOWN) {
            let on: Vec<&str> = Perm::ALL.iter().filter(|p| allowed(&self.denied, &a.id, **p)).map(|p| p.key()).collect();
            let detail = if crate::sandbox::SYSTEM.contains(&a.id.as_str()) {
                "PART OF THE SYSTEM".to_string()
            } else if on.is_empty() {
                "NOTHING ALLOWED".to_string()
            } else if on.len() == Perm::ALL.len() {
                "ALL ALLOWED".to_string()
            } else {
                format!("{} ALLOWED", on.join(" · ").to_uppercase())
            };
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
        if !self.sound.present && self.audio.outputs.is_empty() {
            return text("No speakers or headphones found. Plug them in, or connect Bluetooth headphones in Center → Bluetooth.", SANS, 22.0, alpha(TEXT_2, f));
        }
        let pill = |s: String| -> Element<'_, Message> { fill(text(s, SANS_BOLD, 20.0, alpha(TEXT, f)), alpha(PANEL_2, f)).padding([14, 22]).into() };
        let outs = self.audio.outputs.len();
        let mut col = widget::Column::new()
            .spacing(14)
            .push(self.row_box(0, "Volume", format!("{:.0}%", self.sound.volume * 100.0), slider(self.sound.volume, 900.0, self.row == Some(0), f), f))
            .push(self.row_box(1, "Mute", if self.sound.muted { "ON".into() } else { "OFF".into() }, pill(if self.sound.muted { "Sound is off · Enter".into() } else { "Sound is on · Enter".into() }), f))
            .push(self.row_box(2, "Test", String::new(), pill("Play a chime · Enter".into()), f));
        if let Some(n) = &self.sound_note {
            col = col.push(text(n.clone(), SANS_BOLD, 19.0, alpha(YELLOW, f)));
        }
        col = col.push(space(1.0, 10.0)).push(text("SOUND COMES OUT OF", MONO, 15.0, alpha(TEXT_2, f)));
        for (i, o) in self.audio.outputs.iter().enumerate() {
            col = col.push(self.list_row(3 + i, o.name.clone(), if o.default { "IN USE".into() } else { "ENTER USES IT".into() }, f));
        }
        if !self.audio.inputs.is_empty() {
            col = col
                .push(space(1.0, 10.0))
                .push(text("MICROPHONE", MONO, 15.0, alpha(TEXT_2, f)))
                .push(self.row_box(3 + outs, "Microphone level", format!("{:.0}%", self.audio.mic_volume * 100.0), slider(self.audio.mic_volume, 900.0, self.row == Some(3 + outs), f), f))
                .push(self.row_box(4 + outs, "Microphone", if self.audio.mic_muted { "OFF".into() } else { "ON".into() }, pill(if self.audio.mic_muted { "Muted · Enter".into() } else { "Listening · Enter mutes".into() }), f));
            for (i, d) in self.audio.inputs.iter().enumerate() {
                col = col.push(self.list_row(5 + outs + i, d.name.clone(), if d.default { "IN USE".into() } else { "ENTER USES IT".into() }, f));
            }
        }
        widget::scrollable(col).height(Length::Fill).into()
    }

    fn updates_view(&self, f: f32) -> Element<'_, Message> {
        let label = match self.update {
            Update::Available(_) => "Install now",
            Update::Checking | Update::Installing(_) => "Please wait…",
            _ => "Check for updates",
        };
        let b = fill(text(label, SANS_BOLD, 21.0, alpha(BLACK, f)), alpha(YELLOW, f)).padding([16, 26]);
        widget::Column::new()
            .push(text(format!("Chlorum {}", system::version()), SANS_BOLD, 30.0, alpha(TEXT, f)))
            .push(text(self.update.text(), SANS, 22.0, alpha(TEXT_2, f)))
            .push(match self.update.fraction() {
                Some(x) => progress_line(x, 900.0, f),
                None => space(1.0, 1.0),
            })
            .push(space(1.0, 20.0))
            .push(self.row_box(0, "Updates", String::new(), b.into(), f))
            .spacing(10)
            .into()
    }

    /// Every key that does something everywhere, in one place.
    fn shortcuts_view(&self, f: f32) -> Element<'_, Message> {
        const KEYS: &[(&str, &[(&str, &str)])] = &[
            ("EVERYWHERE", &[
                ("Super", "System menu: apps that are open, notifications, power"),
                ("Super + H", "Home"),
                ("Super + L", "Lock"),
                ("Super + /", "All of niri's shortcuts on screen"),
                ("Alt + Shift", "Next keyboard layout"),
                ("Ctrl + Alt + T", "A terminal"),
            ]),
            ("APPS", &[
                ("Super + ← →", "Two apps side by side: go to the left / right one"),
                ("Super + [ ]", "Two apps side by side: narrower / wider"),
                ("Super + X", "Two apps side by side: swap places"),
                ("Super + F", "Full screen, and back"),
                ("Super + Q", "Close the app in front"),
                ("Super, then X", "Close an app from the menu; X again forces one that hangs"),
            ]),
            ("SCREEN AND SOUND", &[
                ("Print", "Screenshot of the screen (into Library → Screenshots)"),
                ("Shift + Print", "Screenshot of a part you choose"),
                ("Alt + Print", "Screenshot of the app in front"),
                ("Volume keys", "Louder, quieter, mute; the microphone key mutes it"),
                ("Play / Next / Previous", "The music or video playing"),
            ]),
            ("MOUSE WITHOUT A MOUSE", &[
                ("Numpad 8 4 6 2", "Move the pointer (hold to glide)"),
                ("Numpad 5 · / · *", "Click · right click · middle click"),
                ("Numpad 0", "Hold the button down (drag)"),
                ("Numpad − +", "Scroll"),
                ("Super + M", "Arrows move the pointer (laptops); again to stop"),
            ]),
            ("HOME AND LIBRARY", &[
                ("Type", "Find an app (Home) · / finds a file (Library)"),
                ("F", "Library: filter by kind and date, choose the order"),
                ("Space · C", "Library: mark files · copy them to a drive"),
                ("R · M · Del", "Library: rename · move · put in the bin"),
            ]),
        ];
        let mut col = widget::Column::new().spacing(8);
        for (title, keys) in KEYS {
            col = col.push(space(1.0, 8.0)).push(text(*title, MONO, 14.0, alpha(YELLOW, f)));
            for (k, what) in keys.iter() {
                col = col.push(
                    widget::Row::new()
                        .push(widget::container(boxed(text(*k, MONO_BOLD, 15.0, alpha(TEXT, f)), alpha(PANEL_2, f), alpha(LINE, f), 1.0).padding([6, 12])).width(Length::Fixed(300.0)))
                        .push(text(*what, SANS, 19.0, alpha(TEXT_2, f)))
                        .spacing(20)
                        .align_y(Alignment::Center),
                );
            }
        }
        widget::scrollable(col).height(Length::Fill).into()
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
        if matches!(self.update, Update::Available(_) | Update::Installing(_) | Update::Installed(_)) {
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
                        return self.close_window(id);
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
                        self.sub = Sub::Power((i + 1).min(5));
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
                        // sleep: locked first, so it wakes up locked
                        if i == 1 {
                            self.close_menu();
                            return Task::batch([
                                lock(),
                                Task::perform(
                                    async {
                                        tokio::time::sleep(Duration::from_millis(1200)).await;
                                        let _ = tokio::process::Command::new("systemctl").arg("suspend").status().await;
                                    },
                                    |_| cosmic::Action::App(Message::Tick),
                                ),
                            ]);
                        }
                        let i = i - 2;
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
            Some(Named::F10) => self.sub = Sub::Power(0),
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
                self.sub = Sub::Power(0);
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
                let labels = ["Lock", "Sleep", "Log out", "Restart", "Shut down", "Cancel"];
                for (i, l) in labels.iter().enumerate() {
                    let on = i == sel;
                    let armed = on && self.power_armed && (2..=4).contains(&i);
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
            Card::Update => {
                let value = match &self.update {
                    Update::Available(v) | Update::Installed(v) if !v.is_empty() => v.clone(),
                    Update::Installing(Some(p)) if !p.version.is_empty() => p.version.clone(),
                    _ => "New version".into(),
                };
                let below: Element<'_, Message> = match self.update.fraction() {
                    Some(x) => widget::Column::new()
                        .push(text(self.update.text().to_uppercase(), MONO, 11.0, alpha(TEXT_2, f)))
                        .push(progress_line(x, 140.0, f))
                        .spacing(6)
                        .into(),
                    None => text(self.update.text().to_uppercase(), MONO, 12.0, alpha(TEXT_2, f)).into(),
                };
                ("UPDATE", value, below)
            }
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
        // its size is the screen's, so with the zoom
        let (w, h) = ((480.0 * self.zoom).round() as u32, (132.0 * self.zoom).round() as u32);
        if shown { Task::none() } else { Task::batch([set_layer(self.note_id, Layer::Overlay), set_size(self.note_id, Some(w), Some(h))]) }
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

/// Gallery is new in 0.13: on a Home list chosen before, it goes once
/// right after Library (taken off again, it stays off).
fn add_gallery_once() {
    let dir = apps::config_dir().join("chlorum");
    let mark = dir.join("gallery-added");
    let list = dir.join("home-apps");
    if mark.exists() {
        return;
    }
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::fs::write(&mark, "");
    let Ok(text) = std::fs::read_to_string(&list) else { return };
    if text.lines().any(|l| l.trim().trim_end_matches(".desktop") == "chlorum-gallery") {
        return;
    }
    let mut out = vec![];
    let mut added = false;
    for l in text.lines() {
        out.push(l.to_string());
        if !added && l.trim().trim_end_matches(".desktop") == "chlorum-library" {
            out.push("chlorum-gallery".into());
            added = true;
        }
    }
    if !added {
        out.push("chlorum-gallery".into());
    }
    let _ = std::fs::write(&list, out.join("\n") + "\n");
}

/// How far the wallpaper's video is (it is drawn once, then played).
fn wallpaper_progress() -> String {
    let dir = std::env::var_os("XDG_RUNTIME_DIR").map(std::path::PathBuf::from).unwrap_or_else(|| "/tmp".into());
    match std::fs::read_to_string(dir.join("chlorum/wallpaper-render")) {
        Ok(p) => format!("MAKING THE 4K 60 FPS VIDEO OF THIS WALLPAPER · {}% · IT SHOWS BY ITSELF WHEN READY", p.trim()),
        Err(_) => String::new(),
    }
}

/// "English (US)" → "EN", "Ukrainian" → "UK", "French" → "FR".
fn layout_short(name: &str) -> String {
    let n = name.to_lowercase();
    let known = [("english", "EN"), ("ukrainian", "UK"), ("french", "FR"), ("german", "DE"), ("spanish", "ES"), ("italian", "IT"), ("polish", "PL"), ("russian", "RU"), ("belgian", "BE"), ("swiss", "CH")];
    known.iter().find(|(k, _)| n.starts_with(k)).map(|(_, v)| v.to_string()).unwrap_or_else(|| name.chars().take(2).collect::<String>().to_uppercase())
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
