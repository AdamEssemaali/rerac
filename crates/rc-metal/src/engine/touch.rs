//! On-screen controls for touch screens (iOS): a virtual PS2 pad over the frame. The window's touches drive it
//! ([`TouchPad::touch`]); while it is the device in use (`InputState::touch_active`) the input state hands it out as
//! the gamepad and crate::touch_controls draws it from [`TouchPad::shapes`].
//!
//! Layout, in window pixels, scaled by the window's shorter side (`u`): the left stick anywhere in the lower left
//! (it centres where the thumb lands), the four face buttons in the lower right, L1 / L2 and R1 / R2 in the upper
//! corners (clear of an iPhone's notch), SELECT and START at the top centre. A finger that slides from one button to another moves the press with
//! it; the stick keeps its finger until it lifts.

use super::input::{Gamepad, GamepadButton};
use glam::Vec2;

/// A touch's phase (winit's `TouchPhase`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase { Started, Moved, Ended }

/// What a touch holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Control { Stick, Button(GamepadButton), Nothing }

/// What [`TouchPad::shapes`] draws (crate::touch_controls; shaders/shim/touch_controls.wgsl `kind`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShapeKind { StickBase = 0, StickKnob = 1, Cross = 2, Circle = 3, Square = 4, Triangle = 5, Pill = 6 }

/// One shape of the overlay: its centre and half size in window pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shape {
    pub kind: ShapeKind,
    pub centre: Vec2,
    pub half: Vec2,
    pub pressed: bool,
}

/// The stick's travel for full deflection, in `u`.
const STICK_RADIUS: f32 = 1.9;
/// A button's drawn radius and its touch radius, in `u`.
const BUTTON_RADIUS: f32 = 0.8;
const BUTTON_HIT: f32 = 1.1;

#[derive(Clone, Debug, Default)]
pub struct TouchPad {
    /// The window size the layout is made for.
    size: Vec2,
    /// The live touches: id, what it holds, and for the stick its origin.
    touches: Vec<(u64, Control, Vec2)>,
    /// Where the stick's finger is now (with the stick's origin in `touches`).
    stick_at: Option<Vec2>,
    pub pad: Gamepad,
    /// A touch since the input state last asked ([`TouchPad::take_touched`]).
    touched: bool,
}

impl TouchPad {
    fn u(&self) -> f32 { self.size.x.min(self.size.y) * 0.085 }

    fn stick_home(&self) -> Vec2 { let u = self.u(); Vec2::new(3.6 * u, self.size.y - 3.4 * u) }

    /// The buttons: centre, half size (x = y for round ones), shape, button.
    fn buttons(&self) -> Vec<(Vec2, Vec2, ShapeKind, GamepadButton)> {
        let (u, w, h) = (self.u(), self.size.x, self.size.y);
        let face = Vec2::new(w - 3.6 * u, h - 3.4 * u);
        let round = Vec2::splat(BUTTON_RADIUS * u);
        let shoulder = Vec2::new(1.1 * u, 0.48 * u);
        let small = Vec2::new(0.75 * u, 0.32 * u);
        vec![
            (face + Vec2::new(0.0, 1.55 * u), round, ShapeKind::Cross, GamepadButton::South),
            (face + Vec2::new(1.55 * u, 0.0), round, ShapeKind::Circle, GamepadButton::East),
            (face + Vec2::new(-1.55 * u, 0.0), round, ShapeKind::Square, GamepadButton::West),
            (face + Vec2::new(0.0, -1.55 * u), round, ShapeKind::Triangle, GamepadButton::North),
            (Vec2::new(3.3 * u, 1.0 * u), shoulder, ShapeKind::Pill, GamepadButton::LeftTrigger2),
            (Vec2::new(3.3 * u, 2.3 * u), shoulder, ShapeKind::Pill, GamepadButton::LeftTrigger),
            (Vec2::new(w - 3.3 * u, 1.0 * u), shoulder, ShapeKind::Pill, GamepadButton::RightTrigger2),
            (Vec2::new(w - 3.3 * u, 2.3 * u), shoulder, ShapeKind::Pill, GamepadButton::RightTrigger),
            (Vec2::new(w * 0.5 - 1.4 * u, 0.9 * u), small, ShapeKind::Pill, GamepadButton::Select),
            (Vec2::new(w * 0.5 + 1.4 * u, 0.9 * u), small, ShapeKind::Pill, GamepadButton::Start),
        ]
    }

    /// The button under `p`, if any.
    fn button_at(&self, p: Vec2) -> Option<GamepadButton> {
        let slack = (BUTTON_HIT - BUTTON_RADIUS) * self.u();
        self.buttons().into_iter().find(|(c, half, kind, _)| {
            let d = (p - *c).abs();
            match kind {
                ShapeKind::Pill => d.x <= half.x + slack && d.y <= half.y + slack,
                _ => d.length() <= half.x + slack,
            }
        }).map(|(.., b)| b)
    }

    /// The stick's area: the lower left, clear of the shoulder buttons.
    fn in_stick_zone(&self, p: Vec2) -> bool { p.x < self.size.x * 0.45 && p.y > 3.2 * self.u() }

    /// The window's size (physical pixels), before the touches of a frame.
    pub fn resize(&mut self, size: Vec2) { self.size = size; }

    /// One touch event (physical pixels, y down).
    pub fn touch(&mut self, id: u64, phase: Phase, p: Vec2) {
        self.touched = true;
        let i = self.touches.iter().position(|t| t.0 == id);
        match (phase, i) {
            (Phase::Started, None) => {
                let c = match self.button_at(p) {
                    Some(b) => Control::Button(b),
                    None if self.in_stick_zone(p) && self.stick_at.is_none() => {
                        self.stick_at = Some(p);
                        Control::Stick
                    }
                    None => Control::Nothing,
                };
                self.touches.push((id, c, p));
            }
            (Phase::Moved | Phase::Started, Some(i)) => match self.touches[i].1 {
                Control::Stick => self.stick_at = Some(p),
                _ => self.touches[i].1 = self.button_at(p).map_or(Control::Nothing, Control::Button),
            },
            (Phase::Ended, Some(i)) => {
                if self.touches[i].1 == Control::Stick { self.stick_at = None; }
                self.touches.remove(i);
            }
            _ => {}
        }
        self.apply();
    }

    /// The gamepad state from the live touches.
    fn apply(&mut self) {
        let held: Vec<GamepadButton> = self.touches.iter().filter_map(|t| match t.1 { Control::Button(b) => Some(b), _ => None }).collect();
        for (.., b) in self.buttons() {
            if held.contains(&b) { self.pad.buttons.press(b) } else { self.pad.buttons.release(b) }
        }
        self.pad.left = match (self.touches.iter().find(|t| t.1 == Control::Stick), self.stick_at) {
            (Some(&(_, _, origin)), Some(at)) => {
                let v = ((at - origin) / (STICK_RADIUS * self.u())).clamp_length_max(1.0);
                // Y up, as the gamepads'; the gamepads' dead zone.
                let v = Vec2::new(v.x, -v.y);
                if v.length() < 0.1 { Vec2::ZERO } else { v }
            }
            _ => Vec2::ZERO,
        };
    }

    /// Whether a touch came since the last call.
    pub fn take_touched(&mut self) -> bool { std::mem::take(&mut self.touched) }

    /// The overlay.
    pub fn shapes(&self) -> Vec<Shape> {
        if self.size == Vec2::ZERO { return Vec::new(); }
        let u = self.u();
        let stick = self.touches.iter().find(|t| t.1 == Control::Stick).map(|t| t.2);
        let base = stick.unwrap_or_else(|| self.stick_home());
        let knob = base + Vec2::new(self.pad.left.x, -self.pad.left.y) * STICK_RADIUS * u;
        let mut out = vec![
            Shape { kind: ShapeKind::StickBase, centre: base, half: Vec2::splat(STICK_RADIUS * u), pressed: stick.is_some() },
            Shape { kind: ShapeKind::StickKnob, centre: knob, half: Vec2::splat(0.85 * u), pressed: stick.is_some() },
        ];
        for (centre, half, kind, b) in self.buttons() {
            out.push(Shape { kind, centre, half, pressed: self.pad.pressed(b) });
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pad() -> TouchPad {
        let mut t = TouchPad::default();
        t.resize(Vec2::new(2532.0, 1170.0));
        t
    }

    #[test]
    fn buttons_press_slide_and_release() {
        let mut t = pad();
        let cross = t.buttons()[0].0;
        let circle = t.buttons()[1].0;
        t.touch(1, Phase::Started, cross);
        assert!(t.pad.pressed(GamepadButton::South) && t.take_touched() && !t.take_touched());
        t.touch(1, Phase::Moved, circle);
        assert!(!t.pad.pressed(GamepadButton::South) && t.pad.pressed(GamepadButton::East));
        t.touch(1, Phase::Ended, circle);
        assert!(!t.pad.pressed(GamepadButton::East));
    }

    #[test]
    fn stick_centres_on_the_thumb_and_clamps() {
        let mut t = pad();
        let o = Vec2::new(400.0, 900.0);
        t.touch(7, Phase::Started, o);
        assert_eq!(t.pad.left_stick(), Vec2::ZERO);
        // Right and up (screen y down → stick y up), past full travel.
        t.touch(7, Phase::Moved, o + Vec2::new(1000.0, -1000.0));
        let v = t.pad.left_stick();
        assert!((v.length() - 1.0).abs() < 1e-5 && v.x > 0.0 && v.y > 0.0, "{v}");
        // A second finger in the zone does not take the stick; a button still works alongside.
        t.touch(8, Phase::Started, o + Vec2::new(50.0, 0.0));
        t.touch(9, Phase::Started, t.buttons()[3].0);
        assert!(t.pad.pressed(GamepadButton::North) && t.pad.left_stick().length() > 0.99);
        t.touch(7, Phase::Ended, o);
        assert_eq!(t.pad.left_stick(), Vec2::ZERO);
    }

    #[test]
    fn layout_does_not_overlap() {
        let t = pad();
        let b = t.buttons();
        for (i, a) in b.iter().enumerate() {
            assert_eq!(t.button_at(a.0), Some(a.3), "{:?}", a.3);
            assert!(!t.in_stick_zone(a.0), "{:?} inside the stick zone", a.3);
            for c in &b[i + 1..] { assert!((a.0 - c.0).length() > a.1.x.min(c.1.x), "{:?} / {:?}", a.3, c.3); }
        }
        assert!(t.in_stick_zone(t.stick_home()));
        assert_eq!(t.shapes().len(), b.len() + 2);
    }
}
