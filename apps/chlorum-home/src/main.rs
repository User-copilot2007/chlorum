// Chlorum Home: the main menu on the Super key.
// Clock with seconds, date and time zone, and a PS5-style ribbon of apps.
// Fully keyboard driven: arrows move, Enter opens, numbers jump, Esc closes.

use chrono::{Local, Offset, Timelike};
use cosmic::app::{Core, Settings, Task};
use cosmic::iced::{
    self, Alignment, Border, Color, Font, Length, Subscription, event, keyboard,
    font::{Family, Weight},
    window,
};
use cosmic::iced::widget::container as iced_container;
use cosmic::{Application, Element, widget};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

const YELLOW: Color = Color::from_rgb(1.0, 0.839, 0.039);
const PANEL: Color = Color::from_rgb(0.067, 0.067, 0.067);
const LINE: Color = Color::from_rgb(0.15, 0.15, 0.15);
const TEXT: Color = Color::from_rgb(0.96, 0.96, 0.96);
const TEXT_2: Color = Color::from_rgb(0.83, 0.83, 0.83);
const MUTED: Color = Color::from_rgb(0.6, 0.6, 0.6);

const TILE: f32 = 132.0; // resting tile size (logical pixels; COSMIC scales for HiDPI)
const TILE_FOCUS: f32 = 188.0; // focused tile size

const MONO: Font = Font { family: Family::Name("IBM Plex Mono"), weight: Weight::Normal, ..Font::DEFAULT };
const MONO_BOLD: Font = Font { family: Family::Name("IBM Plex Mono"), weight: Weight::Semibold, ..Font::DEFAULT };
const SANS_BOLD: Font = Font { family: Family::Name("IBM Plex Sans"), weight: Weight::Semibold, ..Font::DEFAULT };

/// Apps shown when the owner has not chosen their own list.
const DEFAULT_APPS: &[&str] = &[
    "firefox.desktop",
    "com.system76.CosmicTerm.desktop",
    "com.system76.CosmicFiles.desktop",
    "com.system76.CosmicEdit.desktop",
    "com.system76.CosmicSettings.desktop",
];

fn main() -> cosmic::iced::Result {
    let settings = Settings::default()
        .client_decorations(false)
        .transparent(true)
        .size(iced::Size::new(1600.0, 900.0));
    cosmic::app::run_single_instance::<Home>(settings, Flags)
}

#[derive(Clone, Debug, Default)]
struct Flags;

impl cosmic::app::CosmicFlags for Flags {
    type SubCommand = String;
    type Args = Vec<String>;
}

#[derive(Clone, Debug)]
enum Tile {
    App { name: String, icon: String, exec: String },
    Update,
}

impl Tile {
    fn name(&self) -> &str {
        match self {
            Tile::App { name, .. } => name,
            Tile::Update => "Update Chlorum",
        }
    }
    fn icon(&self) -> &str {
        match self {
            Tile::App { icon, .. } => icon,
            Tile::Update => "system-software-update",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Power {
    LogOut,
    Restart,
    Shutdown,
}

impl Power {
    const ALL: [Power; 3] = [Power::LogOut, Power::Restart, Power::Shutdown];
    fn label(self) -> &'static str {
        match self {
            Power::LogOut => "Log out",
            Power::Restart => "Restart",
            Power::Shutdown => "Shut down",
        }
    }
    fn icon(self) -> &'static str {
        match self {
            Power::LogOut => "system-log-out-symbolic",
            Power::Restart => "system-reboot-symbolic",
            Power::Shutdown => "system-shutdown-symbolic",
        }
    }
    fn osd(self) -> &'static str {
        match self {
            Power::LogOut => "log-out",
            Power::Restart => "restart",
            Power::Shutdown => "shutdown",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Zone {
    Ribbon,
    Power,
}

#[derive(Clone, Debug, PartialEq)]
enum UpdateState {
    Idle,
    Checking,
    UpToDate,
    Updating,
    Done,
    Failed(String),
}

#[derive(Clone, Debug)]
enum Message {
    Tick,
    Frame,
    Key(keyboard::Key),
    Hover(usize),
    Open(usize),
    HoverPower(usize),
    PowerPressed(usize),
    UpdateChecked(Result<bool, String>),
    UpdateFinished(Result<(), String>),
}

struct Home {
    core: Core,
    tiles: Vec<Tile>,
    zone: Zone,
    selected: usize,
    power: usize,
    /// 0.0 = resting, 1.0 = focused, animated per tile
    grow: Vec<f32>,
    last_frame: Instant,
    update: UpdateState,
    user: String,
    /// 0.0 = invisible, 1.0 = fully shown; fades in on open and out on close
    appear: f32,
    closing: bool,
    after_close: Option<After>,
}

/// What to do once the fade-out has finished
#[derive(Clone, Debug)]
enum After {
    Spawn(String),
    Osd(&'static str),
}

impl Application for Home {
    type Executor = cosmic::executor::Default;
    type Flags = Flags;
    type Message = Message;
    const APP_ID: &'static str = "com.chlorum.Home";

    fn core(&self) -> &Core {
        &self.core
    }
    fn core_mut(&mut self) -> &mut Core {
        &mut self.core
    }

    fn init(mut core: Core, _: Flags) -> (Self, Task<Message>) {
        core.window.show_headerbar = false;
        core.window.content_container = false;
        let tiles = load_tiles();
        let mut grow = vec![0.0; tiles.len()];
        if let Some(g) = grow.first_mut() {
            *g = 1.0;
        }
        let home = Home {
            core,
            tiles,
            zone: Zone::Ribbon,
            selected: 0,
            power: 0,
            grow,
            last_frame: Instant::now(),
            update: UpdateState::Idle,
            user: std::env::var("USER").unwrap_or_default(),
            appear: 0.0,
            closing: false,
            after_close: None,
        };
        let task = match home.core.main_window_id() {
            Some(id) => window::set_mode(id, window::Mode::Fullscreen),
            None => Task::none(),
        };
        (home, task)
    }

    // A second press of Super starts another copy, which lands here: close.
    fn dbus_activation(&mut self, _msg: cosmic::dbus_activation::Message) -> Task<Message> {
        self.close(None)
    }

    fn on_escape(&mut self) -> Task<Message> {
        self.close(None)
    }

    fn subscription(&self) -> Subscription<Message> {
        let mut subs = vec![
            iced::time::every(Duration::from_millis(250)).map(|_| Message::Tick),
            event::listen_with(|e, status, _| {
                if std::env::var_os("CHLORUM_DEBUG").is_some() {
                    if let iced::Event::Keyboard(k) = &e {
                        eprintln!("key event {k:?} {status:?}");
                    }
                }
                match e {
                iced::Event::Keyboard(keyboard::Event::KeyPressed { key, .. })
                    if status == event::Status::Ignored =>
                {
                    Some(Message::Key(key))
                }
                _ => None,
            }}),
        ];
        if self.animating() {
            subs.push(iced::time::every(Duration::from_millis(16)).map(|_| Message::Frame));
        }
        Subscription::batch(subs)
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Tick => {}
            Message::Frame => {
                self.step_animation();
                if self.closing && self.appear <= 0.0 {
                    match self.after_close.take() {
                        Some(After::Spawn(exec)) => spawn(&exec),
                        Some(After::Osd(what)) => {
                            let _ = Command::new("cosmic-osd").arg(what).spawn();
                        }
                        None => {}
                    }
                    return iced::exit();
                }
            }
            Message::Hover(i) => {
                self.zone = Zone::Ribbon;
                self.selected = i;
                self.kick();
            }
            Message::HoverPower(i) => {
                self.zone = Zone::Power;
                self.power = i;
                self.kick();
            }
            Message::Open(i) => return self.open(i),
            Message::PowerPressed(i) => return self.power_action(Power::ALL[i]),
            Message::Key(key) => return self.key(key),
            Message::UpdateChecked(Ok(true)) => {
                self.update = UpdateState::Updating;
                return Task::perform(run_update(), |r| cosmic::Action::App(Message::UpdateFinished(r)));
            }
            Message::UpdateChecked(Ok(false)) => self.update = UpdateState::UpToDate,
            Message::UpdateChecked(Err(e)) => self.update = UpdateState::Failed(e),
            Message::UpdateFinished(Ok(())) => self.update = UpdateState::Done,
            Message::UpdateFinished(Err(e)) => self.update = UpdateState::Failed(e),
        }
        Task::none()
    }

    fn view(&self) -> Element<'_, Message> {
        let now = Local::now();
        let space = 28.0;

        // Top: brand left, profile and power right
        let brand = widget::Row::new()
            .push(if self.appear > 0.5 { Element::from(widget::icon::from_name("com.chlorum.Logo").size(36)) } else { widget::Space::new().width(Length::Fixed(36.0)).into() })
            .push(widget::text("C H L O R U M").font(MONO_BOLD).size(18).class(self.fade(YELLOW)))
            .spacing(14)
            .align_y(Alignment::Center);
        let mut right = widget::Row::new()
            .push(widget::text(self.user.to_uppercase()).font(MONO).size(14).class(self.fade(MUTED)))
            .spacing(10)
            .align_y(Alignment::Center);
        for (i, p) in Power::ALL.iter().enumerate() {
            let focused = self.zone == Zone::Power && self.power == i;
            right = right.push(power_button(*p, focused, i, self.appear));
        }
        let top = widget::Row::new()
            .push(brand)
            .push(widget::Space::new().width(Length::Fill))
            .push(right)
            .align_y(Alignment::Center);

        // Clock: HH:MM in white, :SS in yellow
        let clock = widget::Row::new()
            .push(
                widget::text(format!("{:02}:{:02}", now.hour(), now.minute()))
                    .font(MONO_BOLD)
                    .size(156)
                    .class(self.fade(TEXT)),
            )
            .push(widget::text(format!(":{:02}", now.second())).font(MONO_BOLD).size(156).class(self.fade(YELLOW)));
        let date = widget::text(format!(
            "{}  ·  {}",
            now.format("%A, %B %-d, %Y"),
            zone_label(&now)
        ))
        .font(MONO)
        .size(22)
        .class(self.fade(TEXT_2));

        // Ribbon
        let mut ribbon = widget::Row::new().spacing(20).align_y(Alignment::End).height(Length::Fixed(TILE_FOCUS));
        for (i, t) in self.tiles.iter().enumerate() {
            ribbon = ribbon.push(self.tile(i, t));
        }
        let name = match (self.zone, self.tiles.get(self.selected)) {
            (Zone::Ribbon, Some(t)) => t.name().to_string(),
            _ => String::new(),
        };
        let label = widget::text(name).font(SANS_BOLD).size(26).class(self.fade(TEXT));

        let status = widget::text(self.update_text()).font(MONO).size(15).class(self.fade(match self.update {
            UpdateState::Failed(_) => Color::from_rgb(1.0, 0.36, 0.36),
            UpdateState::UpToDate | UpdateState::Done => YELLOW,
            _ => TEXT_2,
        }));

        let hints = widget::Row::new()
            .push(hint("← →", "Move", self.appear))
            .push(hint("↑ ↓", "Power / apps", self.appear))
            .push(hint("ENTER", "Open", self.appear))
            .push(hint("1–9", "Jump", self.appear))
            .push(hint("ESC", "Close", self.appear))
            .spacing(32);

        let content = widget::Column::new()
            .push(top)
            .push(widget::Space::new().height(Length::Fixed(space * 2.0)))
            .push(clock)
            .push(date)
            .push(widget::Space::new().height(Length::Fill))
            .push(widget::text("APPS").font(MONO).size(14).class(self.fade(MUTED)))
            .push(widget::Space::new().height(Length::Fixed(16.0)))
            .push(ribbon)
            .push(widget::Space::new().height(Length::Fixed(14.0)))
            .push(label)
            .push(widget::Space::new().height(Length::Fixed(space)))
            .push(status)
            .push(widget::Space::new().height(Length::Fixed(12.0)))
            .push(hints)
            .padding([56, 72]);

        let bg = self.fade(Color::BLACK);
        widget::container(content)
            .width(Length::Fill)
            .height(Length::Fill)
            .class(cosmic::theme::Container::custom(move |_| iced_container::Style {
                background: Some(bg.into()),
                ..Default::default()
            }))
            .into()
    }
}

impl Home {
    fn animating(&self) -> bool {
        let fade = if self.closing { self.appear > 0.0 } else { self.appear < 1.0 };
        fade || self.grow.iter().enumerate().any(|(i, g)| (g - self.target(i)).abs() > 0.002)
    }

    fn close(&mut self, after: Option<After>) -> Task<Message> {
        if !self.closing {
            self.closing = true;
            self.after_close = after;
            self.kick();
        }
        Task::none()
    }

    /// A colour as it should look at the current point of the fade.
    fn fade(&self, c: Color) -> Color {
        Color { a: c.a * self.appear, ..c }
    }

    fn target(&self, i: usize) -> f32 {
        if self.zone == Zone::Ribbon && i == self.selected { 1.0 } else { 0.0 }
    }

    fn kick(&mut self) {
        self.last_frame = Instant::now();
    }

    // Ease every tile towards its target size: quick at first, soft at the end.
    fn step_animation(&mut self) {
        let dt = self.last_frame.elapsed().as_secs_f32().min(0.05);
        self.last_frame = Instant::now();
        let k = 1.0 - (-dt * 14.0).exp();
        // fade: about 180 ms in, 140 ms out
        if self.closing {
            self.appear = (self.appear - dt / 0.14).max(0.0);
        } else {
            self.appear = (self.appear + dt / 0.18).min(1.0);
        }
        for i in 0..self.grow.len() {
            let t = self.target(i);
            let g = &mut self.grow[i];
            *g += (t - *g) * k;
            if (t - *g).abs() < 0.002 {
                *g = t;
            }
        }
    }

    fn tile<'a>(&'a self, i: usize, t: &'a Tile) -> Element<'a, Message> {
        let g = self.grow.get(i).copied().unwrap_or(0.0);
        let size = TILE + (TILE_FOCUS - TILE) * g;
        let focused = g > 0.5;
        let bg = self.fade(mix(PANEL, YELLOW, g));
        let border = self.fade(mix(LINE, YELLOW, g));
        let badge_color = self.fade(if focused { Color::BLACK } else { MUTED });
        let badge = widget::container(widget::text(format!("{}", i + 1)).font(MONO).size(13).class(badge_color))
            .padding([1, 8])
            .class(cosmic::theme::Container::custom(move |_| iced_container::Style {
                border: Border { color: badge_color, width: 1.0, radius: 2.0.into() },
                ..Default::default()
            }));
        let inner = widget::Column::new()
            .push(badge)
            .push(
                widget::container(if self.appear > 0.5 {
                    Element::from(widget::icon::from_name(t.icon()).size((size * 0.46) as u16))
                } else {
                    widget::Space::new().into()
                })
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .align_x(Alignment::Center)
                    .align_y(Alignment::Center),
            )
            .padding(10);
        let body = widget::container(inner)
            .width(Length::Fixed(size))
            .height(Length::Fixed(size))
            .class(cosmic::theme::Container::custom(move |_| iced_container::Style {
                background: Some(bg.into()),
                border: Border { color: border, width: 1.0, radius: 2.0.into() },
                ..Default::default()
            }));
        widget::mouse_area(body)
            .on_press(Message::Open(i))
            .on_enter(Message::Hover(i))
            .into()
    }

    fn update_text(&self) -> String {
        match &self.update {
            UpdateState::Idle => String::new(),
            UpdateState::Checking => "Checking for a new Chlorum version…".into(),
            UpdateState::UpToDate => "Chlorum is up to date.".into(),
            UpdateState::Updating => "Updating in the background. You can keep working.".into(),
            UpdateState::Done => "Update installed. It starts after the next restart.".into(),
            UpdateState::Failed(e) => format!("The update did not work: {e}. Your current version keeps working."),
        }
    }

    fn key(&mut self, key: keyboard::Key) -> Task<Message> {
        use keyboard::key::Named;
        let n = self.tiles.len();
        match key.as_ref() {
            keyboard::Key::Named(Named::ArrowRight) => match self.zone {
                Zone::Ribbon if n > 0 => self.selected = (self.selected + 1).min(n - 1),
                Zone::Power => self.power = (self.power + 1).min(Power::ALL.len() - 1),
                _ => {}
            },
            keyboard::Key::Named(Named::ArrowLeft) => match self.zone {
                Zone::Ribbon => self.selected = self.selected.saturating_sub(1),
                Zone::Power => self.power = self.power.saturating_sub(1),
            },
            keyboard::Key::Named(Named::ArrowUp) => self.zone = Zone::Power,
            keyboard::Key::Named(Named::ArrowDown) => self.zone = Zone::Ribbon,
            keyboard::Key::Named(Named::Tab) => {
                self.zone = if self.zone == Zone::Ribbon { Zone::Power } else { Zone::Ribbon }
            }
            keyboard::Key::Named(Named::Home) => self.selected = 0,
            keyboard::Key::Named(Named::End) if n > 0 => self.selected = n - 1,
            keyboard::Key::Named(Named::Enter) => {
                return match self.zone {
                    Zone::Ribbon => self.open(self.selected),
                    Zone::Power => self.power_action(Power::ALL[self.power]),
                };
            }
            keyboard::Key::Named(Named::Escape) => return self.close(None),
            keyboard::Key::Character(c) => {
                if let Some(d) = c.chars().next().and_then(|c| c.to_digit(10)) {
                    let i = d as usize;
                    if i >= 1 && i <= n {
                        self.zone = Zone::Ribbon;
                        self.selected = i - 1;
                        return self.open(i - 1);
                    }
                }
            }
            _ => {}
        }
        self.kick();
        Task::none()
    }

    fn open(&mut self, i: usize) -> Task<Message> {
        match self.tiles.get(i) {
            Some(Tile::App { exec, .. }) => {
                let exec = exec.clone();
                self.close(Some(After::Spawn(exec)))
            }
            Some(Tile::Update) => {
                if matches!(self.update, UpdateState::Checking | UpdateState::Updating) {
                    return Task::none();
                }
                self.update = UpdateState::Checking;
                Task::perform(check_update(), |r| cosmic::Action::App(Message::UpdateChecked(r)))
            }
            None => Task::none(),
        }
    }

    fn power_action(&mut self, p: Power) -> Task<Message> {
        // COSMIC's own confirmation with a countdown, so nothing happens by accident.
        self.close(Some(After::Osd(p.osd())))
    }
}

fn power_button<'a>(p: Power, focused: bool, i: usize, appear: f32) -> Element<'a, Message> {
    let fade = |c: Color| Color { a: c.a * appear, ..c };
    let color = fade(if focused { YELLOW } else { TEXT_2 });
    let border = fade(if focused { YELLOW } else { Color::from_rgb(0.23, 0.23, 0.23) });
    let body = widget::container(
        widget::Row::new()
            .push(if appear > 0.5 {
                Element::from(widget::icon::from_name(p.icon()).size(16))
            } else {
                widget::Space::new().width(Length::Fixed(16.0)).into()
            })
            .push(widget::text(p.label()).font(MONO).size(14).class(color))
            .spacing(10)
            .align_y(Alignment::Center),
    )
    .padding([12, 16])
    .class(cosmic::theme::Container::custom(move |_| iced_container::Style {
        border: Border { color: border, width: 1.0, radius: 2.0.into() },
        icon_color: Some(color),
        ..Default::default()
    }));
    widget::mouse_area(body)
        .on_press(Message::PowerPressed(i))
        .on_enter(Message::HoverPower(i))
        .into()
}

fn hint<'a>(key: &'a str, what: &'a str, appear: f32) -> Element<'a, Message> {
    let fade = |c: Color| Color { a: c.a * appear, ..c };
    widget::Row::new()
        .push(widget::text(key).font(MONO).size(14).class(fade(TEXT)))
        .push(widget::text(what).font(MONO).size(14).class(fade(MUTED)))
        .spacing(8)
        .into()
}

fn mix(a: Color, b: Color, t: f32) -> Color {
    Color::from_rgb(a.r + (b.r - a.r) * t, a.g + (b.g - a.g) * t, a.b + (b.b - a.b) * t)
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
    let off = now.offset().fix().local_minus_utc() / 60;
    let sign = if off < 0 { '-' } else { '+' };
    format!("{zone} (UTC{sign}{:02}:{:02})", off.abs() / 60, off.abs() % 60)
}

// ---------------------------------------------------------------- apps

fn load_tiles() -> Vec<Tile> {
    let ids = configured_apps();
    let entries = desktop_entries();
    let mut tiles: Vec<Tile> = ids
        .iter()
        .filter_map(|id| entries.get(id.as_str()).cloned())
        .collect();
    tiles.push(Tile::Update);
    tiles
}

/// The owner's list from ~/.config/chlorum/home-apps (one desktop id per
/// line), or the Chlorum default.
fn configured_apps() -> Vec<String> {
    let path = config_dir().join("chlorum/home-apps");
    match std::fs::read_to_string(path) {
        Ok(s) => s
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .map(str::to_string)
            .collect(),
        Err(_) => DEFAULT_APPS.iter().map(|s| s.to_string()).collect(),
    }
}

fn config_dir() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".config"))
}

fn desktop_entries() -> HashMap<String, Tile> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(h) = std::env::var_os("XDG_DATA_HOME") {
        dirs.push(PathBuf::from(h).join("applications"));
    } else if let Ok(h) = std::env::var("HOME") {
        dirs.push(PathBuf::from(h).join(".local/share/applications"));
    }
    let data = std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/run/current-system/sw/share:/usr/share".into());
    dirs.extend(data.split(':').filter(|d| !d.is_empty()).map(|d| PathBuf::from(d).join("applications")));

    let mut out = HashMap::new();
    for dir in dirs {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let id = e.file_name().to_string_lossy().to_string();
            if !id.ends_with(".desktop") || out.contains_key(&id) {
                continue;
            }
            if let Some(t) = parse_desktop(&e.path()) {
                out.insert(id, t);
            }
        }
    }
    out
}

fn parse_desktop(path: &std::path::Path) -> Option<Tile> {
    let text = std::fs::read_to_string(path).ok()?;
    let (mut name, mut icon, mut exec) = (None, None, None);
    let mut in_main = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_main = line == "[Desktop Entry]";
            continue;
        }
        if !in_main {
            continue;
        }
        if let Some(v) = line.strip_prefix("Name=") {
            name.get_or_insert(v.to_string());
        } else if let Some(v) = line.strip_prefix("Icon=") {
            icon.get_or_insert(v.to_string());
        } else if let Some(v) = line.strip_prefix("Exec=") {
            exec.get_or_insert(v.to_string());
        }
    }
    let exec = exec?
        .split_whitespace()
        .filter(|a| !(a.len() == 2 && a.starts_with('%')))
        .collect::<Vec<_>>()
        .join(" ");
    Some(Tile::App { name: name?, icon: icon.unwrap_or_else(|| "application-x-executable".into()), exec })
}

fn spawn(exec: &str) {
    let _ = Command::new("sh")
        .arg("-c")
        .arg(format!("exec {exec}"))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

// ---------------------------------------------------------------- updates

async fn check_update() -> Result<bool, String> {
    let out = tokio_command("chlorum-update", &["check"]).await?;
    match out.trim() {
        "available" => Ok(true),
        "up-to-date" => Ok(false),
        other => Err(if other.is_empty() { "no answer".into() } else { other.to_string() }),
    }
}

async fn run_update() -> Result<(), String> {
    tokio_command("chlorum-update", &["apply"]).await.map(|_| ())
}

async fn tokio_command(cmd: &str, args: &[&str]) -> Result<String, String> {
    let out = tokio::process::Command::new(cmd)
        .args(args)
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
