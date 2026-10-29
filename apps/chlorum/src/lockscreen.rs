//! `chlorum lock` — the lock screen. A small program of its own (like any
//! screen locker): it locks every screen through the session-lock protocol,
//! asks for the profile password, and only then unlocks and quits. If it
//! crashed, the screen would stay locked, never open.

use crate::atom::Atom;
use crate::ui::*;
use chrono::{Local, Timelike};
use cosmic::app::{Core, Settings, Task};
use cosmic::cctk::sctk::reexports::client::protocol::wl_output::WlOutput;
use cosmic::iced::event::wayland::{OutputEvent, SessionLockEvent};
use cosmic::iced::platform_specific::shell::commands::session_lock;
use cosmic::iced::{self, Alignment, Length, Subscription, event, keyboard, window};
use cosmic::{Application, Element, widget};
use std::time::{Duration, Instant};

pub fn run() -> cosmic::iced::Result {
    // one lock screen at a time
    let flag = crate::apps::runtime_dir().join("locked");
    if let Ok(pid) = std::fs::read_to_string(&flag) {
        let comm = std::fs::read_to_string(format!("/proc/{}/comm", pid.trim())).unwrap_or_default();
        if comm.trim() == "chlorum" {
            return Ok(());
        }
    }
    let _ = std::fs::create_dir_all(crate::apps::runtime_dir());
    let _ = std::fs::write(&flag, std::process::id().to_string());
    let settings = Settings::default().scale_factor(crate::ui::screen_zoom()).no_main_window(true).default_font(SANS).default_icon_theme("Papirus-Dark");
    let r = cosmic::app::run::<Lock>(settings, ());
    let _ = std::fs::remove_file(flag);
    r
}

#[derive(Clone, Debug)]
pub enum Message {
    Tick,
    Frame,
    Output(WlOutput),
    /// a screen went away (unplugged, or a monitor switched off at its button)
    OutputGone(WlOutput),
    Go,
    Lock(SessionLockEvent),
    Submit,
    /// a key and the text it types
    Key(keyboard::Key, Option<String>),
    Checked(Result<(), String>),
    Quit,
}

pub struct Lock {
    core: Core,
    outputs: Vec<WlOutput>,
    asked: bool,
    /// the compositor said the session is locked
    locked: bool,
    surfaces: Vec<window::Id>,
    password: String,
    busy: bool,
    error: Option<String>,
    user: String,
    name: String,
    started: Instant,
}

impl Lock {
    /// Fresh lock surfaces on every screen there is now. A screen that was
    /// switched off and on comes back as a new one; a surface made for the
    /// old one would never be drawn again (the clock stood still and keys
    /// seemed to do nothing). The session stays locked meanwhile.
    fn rebuild(&mut self) -> Task<Message> {
        let mut t: Vec<Task<Message>> = self.surfaces.drain(..).map(session_lock::destroy_lock_surface).collect();
        for o in self.outputs.clone() {
            let id = window::Id::unique();
            self.surfaces.push(id);
            t.push(session_lock::get_lock_surface(id, o));
        }
        Task::batch(t)
    }
}

impl Application for Lock {
    type Executor = cosmic::executor::Default;
    type Flags = ();
    type Message = Message;
    const APP_ID: &'static str = "com.chlorum.Lock";

    fn core(&self) -> &Core {
        &self.core
    }
    fn core_mut(&mut self) -> &mut Core {
        &mut self.core
    }

    fn init(core: Core, _: ()) -> (Self, Task<Message>) {
        let l = Lock {
            core,
            outputs: vec![],
            asked: false,
            locked: false,
            surfaces: vec![],
            password: String::new(),
            busy: false,
            error: None,
            user: std::env::var("USER").unwrap_or_default(),
            name: crate::system::display_name(),
            started: Instant::now(),
        };
        // screens are announced right after start; lock once they are known
        let go = Task::perform(tokio::time::sleep(Duration::from_millis(300)), |_| cosmic::Action::App(Message::Go));
        // the password is typed in the first layout (English): a lock
        // started while Ukrainian was on would turn it into other letters
        let latin = Task::perform(
            crate::niri::run(vec![niri_ipc::Action::SwitchLayout { layout: niri_ipc::LayoutSwitchTarget::Index(0) }]),
            |_| cosmic::Action::App(Message::Tick),
        );
        (l, Task::batch([go, latin]))
    }

    fn subscription(&self) -> Subscription<Message> {
        let mut s = vec![
            iced::time::every(Duration::from_millis(250)).map(|_| Message::Tick),
            event::listen_with(|e, _, _| match e {
                // the lock screen reads the keys itself: no field has to be
                // chosen first, whatever is typed is the password
                iced::Event::Keyboard(keyboard::Event::KeyPressed { key, text, modifiers, .. }) => {
                    let typed = text.map(|t| t.to_string()).filter(|t| !t.chars().any(char::is_control) && !modifiers.control() && !modifiers.logo());
                    Some(Message::Key(key, typed))
                }
                iced::Event::PlatformSpecific(iced::event::PlatformSpecific::Wayland(w)) => match w {
                    // a screen switched back on may only report new details
                    iced::event::wayland::Event::Output(OutputEvent::Created(_) | OutputEvent::InfoUpdate(_), o) => Some(Message::Output(o)),
                    iced::event::wayland::Event::Output(OutputEvent::Removed, o) => Some(Message::OutputGone(o)),
                    iced::event::wayland::Event::SessionLock(ev) => Some(Message::Lock(ev)),
                    _ => None,
                },
                _ => None,
            }),
        ];
        if self.busy {
            s.push(iced::time::every(Duration::from_millis(16)).map(|_| Message::Frame));
        }
        Subscription::batch(s)
    }

    fn update(&mut self, m: Message) -> Task<Message> {
        match m {
            Message::Tick | Message::Frame => {}
            Message::Output(o) => {
                if !self.outputs.contains(&o) {
                    self.outputs.push(o);
                    // a screen plugged in (or switched back on) while locked
                    if self.locked {
                        eprintln!("chlorum lock: a screen came back: drawing the lock screen anew");
                        return self.rebuild();
                    }
                }
            }
            Message::OutputGone(o) => {
                self.outputs.retain(|x| *x != o);
                if self.locked {
                    eprintln!("chlorum lock: a screen went away: drawing the lock screen anew");
                    return self.rebuild();
                }
            }
            Message::Go => {
                if self.outputs.is_empty() {
                    if self.started.elapsed() > Duration::from_secs(3) {
                        eprintln!("chlorum lock: no screens");
                        return Task::done(cosmic::Action::App(Message::Quit));
                    }
                    return Task::perform(tokio::time::sleep(Duration::from_millis(200)), |_| cosmic::Action::App(Message::Go));
                }
                if !self.asked {
                    self.asked = true;
                    return session_lock::lock();
                }
            }
            Message::Lock(SessionLockEvent::Locked) => {
                if !self.locked {
                    self.locked = true;
                    return self.rebuild();
                }
            }
            Message::Lock(SessionLockEvent::Focused(..)) => {}
            Message::Lock(SessionLockEvent::Finished) | Message::Lock(SessionLockEvent::NotSupported) => {
                eprintln!("chlorum lock: the compositor refused to lock");
                return Task::done(cosmic::Action::App(Message::Quit));
            }
            Message::Lock(SessionLockEvent::Unlocked) => return Task::done(cosmic::Action::App(Message::Quit)),
            Message::Lock(_) => {}
            Message::Key(k, typed) => {
                use keyboard::key::Named;
                if self.busy {
                    return Task::none();
                }
                match k.as_ref() {
                    keyboard::Key::Named(Named::Enter) => return self.update(Message::Submit),
                    keyboard::Key::Named(Named::Escape) => self.password.clear(),
                    keyboard::Key::Named(Named::Backspace) => {
                        self.password.pop();
                    }
                    _ => {
                        if let Some(t) = typed {
                            self.password.push_str(&t);
                            self.error = None;
                        }
                    }
                }
            }
            Message::Submit => {
                if self.busy || self.password.is_empty() {
                    return Task::none();
                }
                self.busy = true;
                self.started = Instant::now();
                let (u, p) = (self.user.clone(), self.password.clone());
                return Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || crate::lock::authenticate(&u, &p))
                            .await
                            .unwrap_or_else(|e| Err(e.to_string()))
                    },
                    |r| cosmic::Action::App(Message::Checked(r)),
                );
            }
            Message::Checked(Ok(())) => {
                self.busy = false;
                self.locked = false;
                let mut t: Vec<Task<Message>> = self.surfaces.drain(..).map(session_lock::destroy_lock_surface).collect();
                t.push(session_lock::unlock());
                t.push(Task::perform(tokio::time::sleep(Duration::from_millis(400)), |_| cosmic::Action::App(Message::Quit)));
                return Task::batch(t);
            }
            Message::Checked(Err(e)) => {
                self.busy = false;
                self.password.clear();
                self.error = Some(e);
            }
            Message::Quit => {
                let _ = std::fs::remove_file(crate::apps::runtime_dir().join("locked"));
                std::process::exit(0);
            }
        }
        Task::none()
    }

    fn view(&self) -> Element<'_, Message> {
        widget::Space::new().into()
    }

    fn view_window(&self, id: window::Id) -> Element<'_, Message> {
        let main = self.surfaces.first() == Some(&id);
        let now = Local::now();
        let mut col = widget::Column::new()
            .align_x(Alignment::Center)
            .spacing(18)
            .push(vfill())
            .push(widget::icon::from_name("com.chlorum.Logo").size(300))
            .push(
                widget::Row::new()
                    .push(text(format!("{:02}:{:02}", now.hour(), now.minute()), MONO_BOLD, 88.0, TEXT))
                    .push(text(format!(":{:02}", now.second()), MONO_BOLD, 88.0, YELLOW)),
            )
            .push(text(now.format("%A  ·  %-d %B").to_string().to_uppercase(), MONO, 21.0, TEXT_2));
        if main {
            col = col.push(space(1.0, 30.0)).push(text(self.name.clone(), SANS_BOLD, 26.0, TEXT));
            if self.busy {
                col = col.push(Atom::view(90.0, self.started));
            } else {
                // dots for what is typed, a bar where the next letter goes
                let shown = if self.password.is_empty() {
                    text("Type your password", SANS, 26.0, MUTED)
                } else {
                    text(format!("{}▏", "•".repeat(self.password.chars().count().min(40))), MONO_BOLD, 30.0, TEXT)
                };
                let field = fill(shown, cosmic::iced::Color::from_rgb(0.05, 0.05, 0.05)).padding([20, 24]).width(Length::Fixed(560.0));
                col = col.push(bracket(field, 1.0, 10.0));
            }
            if let Some(e) = &self.error {
                col = col.push(text(e.clone(), SANS_BOLD, 21.0, RED));
            }
            col = col.push(vfill()).push(hints(&[("ENTER", "Unlock", true), ("ESC", "Clear", false)], 1.0));
        } else {
            col = col.push(vfill());
        }
        fill(col.padding([48, 72]).width(Length::Fill), BLACK).width(Length::Fill).height(Length::Fill).into()
    }
}
