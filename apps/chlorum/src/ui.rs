//! The Chlorum look: colours, type and the shared pieces every screen uses.
//! Sizes are logical pixels; on a 24" 4K screen they are drawn at 2x.

use cosmic::iced::widget::container as iced_container;
use cosmic::iced::{
    Alignment, Background, Border, Color, Font, Length,
    font::{Family, Weight},
};
use cosmic::{Element, widget};

pub const YELLOW: Color = Color::from_rgb(1.0, 0.839, 0.039);
pub const BLACK: Color = Color::BLACK;
pub const PANEL: Color = Color::from_rgb(0.078, 0.078, 0.078);
pub const PANEL_2: Color = Color::from_rgb(0.11, 0.11, 0.11);
pub const LINE: Color = Color::from_rgb(0.2, 0.2, 0.2);
pub const TEXT: Color = Color::from_rgb(0.96, 0.96, 0.96);
pub const TEXT_2: Color = Color::from_rgb(0.64, 0.64, 0.64);
pub const MUTED: Color = Color::from_rgb(0.42, 0.42, 0.42);
pub const RED: Color = Color::from_rgb(1.0, 0.42, 0.29);

pub const MONO: Font = Font { family: Family::Name("IBM Plex Mono"), weight: Weight::Normal, ..Font::DEFAULT };
pub const MONO_BOLD: Font = Font { family: Family::Name("IBM Plex Mono"), weight: Weight::Semibold, ..Font::DEFAULT };
pub const SANS: Font = Font { family: Family::Name("IBM Plex Sans"), weight: Weight::Normal, ..Font::DEFAULT };
pub const SANS_BOLD: Font = Font { family: Family::Name("IBM Plex Sans"), weight: Weight::Semibold, ..Font::DEFAULT };

pub fn alpha(c: Color, a: f32) -> Color {
    Color { a: c.a * a, ..c }
}

pub fn mix(a: Color, b: Color, t: f32) -> Color {
    Color {
        r: a.r + (b.r - a.r) * t,
        g: a.g + (b.g - a.g) * t,
        b: a.b + (b.b - a.b) * t,
        a: a.a + (b.a - a.a) * t,
    }
}

/// Eases `value` towards `target`: quick at first, soft at the end.
pub fn ease(value: &mut f32, target: f32, dt: f32, speed: f32) -> bool {
    let k = 1.0 - (-dt * speed).exp();
    *value += (target - *value) * k;
    if (target - *value).abs() < 0.002 {
        *value = target;
        false
    } else {
        true
    }
}

pub fn text<'a, M: 'a>(s: impl Into<String>, font: Font, size: f32, color: Color) -> Element<'a, M> {
    widget::text(s.into()).font(font).size(size).class(color).into()
}

/// A plain filled box.
pub fn fill<'a, M: 'a>(content: impl Into<Element<'a, M>>, bg: Color) -> widget::Container<'a, M, cosmic::Theme> {
    widget::container(content).class(cosmic::theme::Container::custom(move |_| iced_container::Style {
        background: Some(Background::Color(bg)),
        ..Default::default()
    }))
}

pub fn boxed<'a, M: 'a>(
    content: impl Into<Element<'a, M>>,
    bg: Color,
    border: Color,
    width: f32,
) -> widget::Container<'a, M, cosmic::Theme> {
    widget::container(content).class(cosmic::theme::Container::custom(move |_| iced_container::Style {
        background: Some(Background::Color(bg)),
        border: Border { color: border, width, radius: 0.0.into() },
        ..Default::default()
    }))
}

pub fn space<'a, M: 'a>(w: f32, h: f32) -> Element<'a, M> {
    widget::Space::new().width(Length::Fixed(w)).height(Length::Fixed(h)).into()
}

pub fn hfill<'a, M: 'a>() -> Element<'a, M> {
    widget::Space::new().width(Length::Fill).into()
}

pub fn vfill<'a, M: 'a>() -> Element<'a, M> {
    widget::Space::new().height(Length::Fill).into()
}

/// The Chlorum focus mark: yellow corners that hug the chosen element.
/// `on` fades them in and out; `pad` is the room between element and corners.
pub fn bracket<'a, M: 'a>(content: impl Into<Element<'a, M>>, on: f32, pad: f32) -> Element<'a, M> {
    let c = alpha(YELLOW, on.clamp(0.0, 1.0));
    let arm = 22.0;
    let th = 4.0;
    let bar = move |w: f32, h: f32| -> Element<'a, M> {
        fill(space::<M>(w, h), c).width(Length::Fixed(w)).height(Length::Fixed(h)).into()
    };
    let corner = move |top: bool, left: bool| -> Element<'a, M> {
        let h_bar = widget::container(bar(arm, th))
            .width(Length::Fixed(arm))
            .height(Length::Fixed(arm))
            .align_x(if left { Alignment::Start } else { Alignment::End })
            .align_y(if top { Alignment::Start } else { Alignment::End });
        let v_bar = widget::container(bar(th, arm))
            .width(Length::Fixed(arm))
            .height(Length::Fixed(arm))
            .align_x(if left { Alignment::Start } else { Alignment::End })
            .align_y(if top { Alignment::Start } else { Alignment::End });
        widget::container(cosmic::iced::widget::stack![h_bar, v_bar])
            .width(Length::Fixed(arm))
            .height(Length::Fixed(arm))
            .into()
    };
    let corners = widget::Column::new()
        .push(widget::Row::new().push(corner(true, true)).push(hfill()).push(corner(true, false)))
        .push(vfill())
        .push(widget::Row::new().push(corner(false, true)).push(hfill()).push(corner(false, false)))
        .width(Length::Fill)
        .height(Length::Fill);
    let inner = widget::container(content).padding(pad);
    cosmic::iced::widget::stack![inner, corners].into()
}

/// A key cap with what it does, for the hint line at the bottom of a screen.
pub fn hint<'a, M: 'a>(key: &str, what: &str, strong: bool, fade: f32) -> Element<'a, M> {
    let kc = if strong { YELLOW } else { TEXT };
    let cap = boxed(text(key.to_string(), MONO_BOLD, 15.0, alpha(kc, fade)), Color::TRANSPARENT, alpha(if strong { YELLOW } else { TEXT_2 }, fade), 1.5)
        .padding([3, 9]);
    widget::Row::new()
        .push(cap)
        .push(text(what.to_string(), MONO, 15.0, alpha(TEXT_2, fade)))
        .spacing(10)
        .align_y(Alignment::Center)
        .into()
}

pub fn hints<'a, M: 'a>(items: &[(&str, &str, bool)], fade: f32) -> Element<'a, M> {
    let mut row = widget::Row::new().spacing(34).align_y(Alignment::Center);
    for (k, w, s) in items {
        row = row.push(hint(k, w, *s, fade));
    }
    row.into()
}

/// A horizontal slider drawn in the Chlorum style: 0.0..=1.0.
pub fn slider<'a, M: 'a>(value: f32, width: f32, active: bool, fade: f32) -> Element<'a, M> {
    let v = value.clamp(0.0, 1.0);
    let knob = 22.0;
    let track_h = 8.0;
    let filled = (width - knob) * v;
    let track = widget::Row::new()
        .push(fill(space::<M>(filled + knob / 2.0, track_h), alpha(YELLOW, fade)))
        .push(fill(space::<M>(width - filled - knob / 2.0, track_h), alpha(LINE, fade)));
    let knob_el = widget::Row::new()
        .push(space::<M>(filled, knob))
        .push(fill(space::<M>(knob, knob), alpha(if active { TEXT } else { YELLOW }, fade)));
    let track = widget::container(track).height(Length::Fixed(knob)).align_y(Alignment::Center);
    cosmic::iced::widget::stack![track, knob_el].into()
}
