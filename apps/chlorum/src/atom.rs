//! The Chlorum "please wait": a chlorine atom. 17 electrons on shells of
//! 2, 8 and 7; the eighth place on the outer shell stays open and blinks.

use crate::ui::{MONO_BOLD, YELLOW};
use cosmic::iced::widget::canvas::{self, Frame, Geometry, Path, Stroke};
use cosmic::iced::{Color, Length, Pixels, Point, Rectangle, alignment, mouse};
use cosmic::Element;
use std::f32::consts::TAU;
use std::time::Instant;

pub struct Atom {
    started: Instant,
}

impl Atom {
    pub fn view<'a, M: 'a>(size: f32, started: Instant) -> Element<'a, M> {
        canvas::Canvas::new(Atom { started }).width(Length::Fixed(size)).height(Length::Fixed(size)).into()
    }
}

impl<M> canvas::Program<M, cosmic::Theme, cosmic::Renderer> for Atom {
    type State = ();

    fn draw(
        &self,
        _state: &(),
        renderer: &cosmic::Renderer,
        _theme: &cosmic::Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry<cosmic::Renderer>> {
        let mut frame = Frame::new(renderer, bounds.size());
        let c = frame.center();
        let s = bounds.width.min(bounds.height);
        let t = self.started.elapsed().as_secs_f32();
        let ring = Color::from_rgb(0.2, 0.2, 0.2);
        // (radius, places, electrons, turns per second)
        let shells = [(0.20, 2, 2, 0.62), (0.33, 8, 8, -0.31), (0.47, 8, 7, 0.19)];
        for (r, places, electrons, speed) in shells {
            let r = r * s;
            frame.stroke(&Path::circle(c, r), Stroke::default().with_color(ring).with_width((s / 110.0).max(1.0)));
            for i in 0..places {
                let a = t * speed * TAU + i as f32 / places as f32 * TAU - TAU / 4.0;
                let p = Point::new(c.x + a.cos() * r, c.y + a.sin() * r);
                if i < electrons {
                    frame.fill(&Path::circle(p, s * 0.032), YELLOW);
                } else {
                    let blink = 0.35 + 0.65 * (0.5 + 0.5 * (t * 3.5).sin());
                    frame.stroke(
                        &Path::circle(p, s * 0.032),
                        Stroke::default().with_color(Color { a: blink, ..YELLOW }).with_width((s / 90.0).max(1.0)),
                    );
                }
            }
        }
        frame.fill(&Path::circle(c, s * 0.125), YELLOW);
        frame.fill_text(canvas::Text {
            content: "Cl".into(),
            position: c,
            color: Color::BLACK,
            size: Pixels(s * 0.12),
            font: MONO_BOLD,
            align_x: cosmic::iced::core::text::Alignment::Center,
            align_y: alignment::Vertical::Center,
            ..Default::default()
        });
        vec![frame.into_geometry()]
    }
}
