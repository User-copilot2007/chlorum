//! `chlorum ask <app-id> <name> <icon> <permission>...` — the window that
//! asks, like a phone does, whether an app may use something: one question
//! at a time, Allow or Don't allow. It sits above everything and takes the
//! keyboard until it is answered, then prints the answers
//! ("network=on camera=off") and quits.
//!
//! `chlorum run` starts it the first time an app starts (see sandbox.rs).

use crate::sandbox::Perm;
use crate::ui::*;
use cosmic::app::{Core, Settings, Task};
use cosmic::iced::platform_specific::runtime::wayland::layer_surface::{IcedOutput, SctkLayerSurfaceSettings};
use cosmic::iced::platform_specific::shell::commands::layer_surface::{Anchor, KeyboardInteractivity, Layer, get_layer_surface};
use cosmic::iced::{self, Alignment, Length, Subscription, event, keyboard, window};
use cosmic::{Application, Element, widget};
use std::time::{Duration, Instant};

/// Asks in a window of its own (a separate process, so `chlorum run` can
/// go on to become the app) and returns the answers.
pub fn ask(app: &str, name: &str, icon: &str, perms: &[Perm]) -> Result<Vec<(Perm, bool)>, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let out = std::process::Command::new(exe)
        .arg("ask")
        .arg(app)
        .arg(name)
        .arg(icon)
        .args(perms.iter().map(|p| p.key()))
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::inherit())
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err("the question was not answered".into());
    }
    let answers = parse_answers(&String::from_utf8_lossy(&out.stdout));
    if answers.len() != perms.len() {
        return Err("the question was not answered".into());
    }
    Ok(answers)
}

/// Is a question on screen? The shell then gives up the keyboard (it
/// closes Home and the menu and does not open them) until it is answered.
pub fn asking() -> bool {
    crate::sandbox::asking()
}

fn done(code: i32) -> ! {
    std::process::exit(code)
}

fn parse_answers(s: &str) -> Vec<(Perm, bool)> {
    s.split_whitespace()
        .filter_map(|kv| kv.split_once('='))
        .filter_map(|(k, v)| Some((Perm::from_key(k)?, v == "on")))
        .collect()
}

pub struct Flags {
    name: String,
    icon: String,
    perms: Vec<Perm>,
}

pub fn run(args: &[String]) -> Result<(), String> {
    let (Some(_app), Some(name), Some(icon)) = (args.first(), args.get(1), args.get(2)) else {
        return Err("usage: chlorum ask <app-id> <name> <icon> <permission>...".into());
    };
    let perms: Vec<Perm> = args[3..].iter().filter_map(|k| Perm::from_key(k)).collect();
    if perms.is_empty() {
        return Ok(());
    }
    let settings = Settings::default().no_main_window(true).default_font(SANS).default_icon_theme("Papirus-Dark");
    let r = cosmic::app::run::<Ask>(settings, Flags { name: name.clone(), icon: icon.clone(), perms }).map_err(|e| e.to_string());
    r?;
    // only reached when the window closed without answers
    Err("not answered".into())
}

#[derive(Clone, Debug)]
pub enum Message {
    Key(keyboard::Key),
    Frame,
    TimeUp,
}

pub struct Ask {
    core: Core,
    flags: Flags,
    /// which question is shown
    at: usize,
    /// the highlighted button: true = Allow
    allow: bool,
    answers: Vec<(Perm, bool)>,
    shown: Instant,
    fade: f32,
}

impl Ask {
    fn answer(&mut self, on: bool) -> Task<Message> {
        // a key held from before the window came up does not answer
        if self.shown.elapsed() < Duration::from_millis(350) {
            return Task::none();
        }
        let p = self.flags.perms[self.at];
        self.answers.push((p, on));
        if self.at + 1 < self.flags.perms.len() {
            self.at += 1;
            self.allow = false;
            self.shown = Instant::now();
            return Task::none();
        }
        let line: Vec<String> = self.answers.iter().map(|(p, on)| format!("{}={}", p.key(), if *on { "on" } else { "off" })).collect();
        println!("{}", line.join(" "));
        done(0);
    }
}

impl Application for Ask {
    type Executor = cosmic::executor::Default;
    type Flags = Flags;
    type Message = Message;
    const APP_ID: &'static str = "com.chlorum.Ask";

    fn core(&self) -> &Core {
        &self.core
    }
    fn core_mut(&mut self) -> &mut Core {
        &mut self.core
    }

    fn init(core: Core, flags: Flags) -> (Self, Task<Message>) {
        let surface = window::Id::unique();
        let a = Ask { core, flags, at: 0, allow: false, answers: vec![], shown: Instant::now(), fade: 0.0 };
        let open = get_layer_surface(SctkLayerSurfaceSettings {
            id: surface,
            layer: Layer::Overlay,
            keyboard_interactivity: KeyboardInteractivity::Exclusive,
            anchor: Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT,
            output: IcedOutput::Active,
            namespace: "chlorum-ask".into(),
            size: Some((None, None)),
            exclusive_zone: -1,
            size_limits: iced::Limits::NONE.min_width(1.0).min_height(1.0).max_width(100_000.0).max_height(100_000.0),
            ..Default::default()
        });
        // nobody there to answer: the app starts with nothing new allowed
        let give_up = Task::perform(tokio::time::sleep(Duration::from_secs(600)), |_| cosmic::Action::App(Message::TimeUp));
        (a, Task::batch([open, give_up]))
    }

    fn subscription(&self) -> Subscription<Message> {
        let mut s = vec![event::listen_with(|e, _, _| match e {
            iced::Event::Keyboard(keyboard::Event::KeyPressed { key, .. }) => Some(Message::Key(key)),
            _ => None,
        })];
        if self.fade < 1.0 {
            s.push(iced::time::every(Duration::from_millis(16)).map(|_| Message::Frame));
        }
        Subscription::batch(s)
    }

    fn update(&mut self, m: Message) -> Task<Message> {
        match m {
            Message::Frame => {
                self.fade = (self.fade + 0.12).min(1.0);
            }
            Message::TimeUp => done(1),
            Message::Key(k) => {
                use keyboard::key::Named;
                match k.as_ref() {
                    keyboard::Key::Named(Named::ArrowLeft | Named::ArrowUp) => self.allow = true,
                    keyboard::Key::Named(Named::ArrowRight | Named::ArrowDown) => self.allow = false,
                    keyboard::Key::Named(Named::Tab) => self.allow = !self.allow,
                    keyboard::Key::Named(Named::Enter) => return self.answer(self.allow),
                    keyboard::Key::Named(Named::Escape) => return self.answer(false),
                    keyboard::Key::Character(c) if c == " " => return self.answer(self.allow),
                    keyboard::Key::Character(c) => match c.to_lowercase().as_str() {
                        "a" | "y" => return self.answer(true),
                        "d" | "n" => return self.answer(false),
                        _ => {}
                    },
                    _ => {}
                }
            }
        }
        Task::none()
    }

    fn view(&self) -> Element<'_, Message> {
        widget::Space::new().into()
    }

    fn view_window(&self, _: window::Id) -> Element<'_, Message> {
        let f = self.fade;
        let p = self.flags.perms[self.at];
        let n = self.flags.perms.len();
        let button = |label: &'static str, on: bool| -> Element<'_, Message> {
            let b = fill(
                widget::container(text(label, SANS_BOLD, 22.0, alpha(if on { BLACK } else { TEXT }, f))).center_x(Length::Fill),
                alpha(if on { YELLOW } else { PANEL_2 }, f),
            )
            .padding([16, 20])
            .width(Length::Fixed(300.0));
            bracket(b, if on { f } else { 0.0 }, 6.0)
        };
        let mut card = widget::Column::new()
            .spacing(14)
            .align_x(Alignment::Center)
            .push(widget::icon::from_name(self.flags.icon.as_str()).size(96))
            .push(text(self.flags.name.to_uppercase(), MONO_BOLD, 15.0, alpha(TEXT_2, f)))
            .push(
                widget::container(text(p.question(&self.flags.name), SANS_BOLD, 30.0, alpha(TEXT, f)))
                    .width(Length::Fixed(720.0))
                    .center_x(Length::Fixed(720.0)),
            )
            .push(text(format!("If not: {}", p.off_means()), SANS, 19.0, alpha(TEXT_2, f)))
            .push(space(1.0, 16.0))
            .push(widget::Row::new().spacing(28).push(button("Allow", self.allow)).push(button("Don't allow", !self.allow)))
            .push(space(1.0, 8.0))
            .push(text("You can change this later in Center → Apps.", SANS, 16.0, alpha(MUTED, f)));
        if n > 1 {
            card = card.push(text(format!("{} OF {}", self.at + 1, n), MONO, 14.0, alpha(TEXT_2, f)));
        }
        let card = fill(card.padding([40, 56]), alpha(PANEL, f)).width(Length::Fixed(880.0));
        let col = widget::Column::new()
            .align_x(Alignment::Center)
            .push(vfill())
            .push(card)
            .push(vfill())
            .push(hints(&[("← →", "Choose", false), ("ENTER", "OK", true), ("A", "Allow", false), ("D", "Don't allow", false)], f))
            .padding([48, 72])
            .width(Length::Fill);
        fill(col, alpha(BLACK, 0.82 * f)).width(Length::Fill).height(Length::Fill).into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers() {
        assert_eq!(parse_answers("network=on camera=off junk\n"), vec![(Perm::Network, true), (Perm::Camera, false)]);
    }
}
