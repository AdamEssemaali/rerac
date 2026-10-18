//! Device input with Bevy's shapes: `ButtonInput<KeyCode | MouseButton>`, accumulated mouse motion and scroll, and
//! gamepads with Bevy's button and stick names (macOS: gilrs; iOS: Apple's GameController framework, which also gives
//! the hardware keyboard, and the on-screen pad of crate::engine::touch). The window events feed it (crate::app);
//! [`InputState::begin_frame`] clears the per-frame parts as Bevy's `InputSystems` do at the start of `PreUpdate`.

use glam::Vec2;
use std::collections::HashSet;
use std::hash::Hash;

pub use winit::event::MouseButton;
pub use winit::keyboard::KeyCode;

/// Bevy's `ButtonInput<T>`.
#[derive(Clone, Debug)]
pub struct ButtonInput<T: Copy + Eq + Hash> {
    pressed: HashSet<T>,
    just_pressed: HashSet<T>,
    just_released: HashSet<T>,
}

impl<T: Copy + Eq + Hash> Default for ButtonInput<T> {
    fn default() -> Self { ButtonInput { pressed: HashSet::new(), just_pressed: HashSet::new(), just_released: HashSet::new() } }
}

impl<T: Copy + Eq + Hash> ButtonInput<T> {
    pub fn press(&mut self, b: T) {
        if self.pressed.insert(b) { self.just_pressed.insert(b); }
    }

    pub fn release(&mut self, b: T) {
        if self.pressed.remove(&b) { self.just_released.insert(b); }
    }

    pub fn release_all(&mut self) {
        let all: Vec<T> = self.pressed.iter().copied().collect();
        for b in all { self.release(b); }
    }

    pub fn pressed(&self, b: T) -> bool { self.pressed.contains(&b) }
    pub fn just_pressed(&self, b: T) -> bool { self.just_pressed.contains(&b) }
    pub fn just_released(&self, b: T) -> bool { self.just_released.contains(&b) }
    pub fn any_pressed(&self, bs: impl IntoIterator<Item = T>) -> bool { bs.into_iter().any(|b| self.pressed(b)) }
    pub fn any_just_pressed(&self, bs: impl IntoIterator<Item = T>) -> bool { bs.into_iter().any(|b| self.just_pressed(b)) }
    pub fn get_pressed(&self) -> impl Iterator<Item = &T> { self.pressed.iter() }
    pub fn get_just_pressed(&self) -> impl Iterator<Item = &T> { self.just_pressed.iter() }

    /// Clears everything (Bevy's `reset_all`; rc-engine's frame-exact mode drops live input with it).
    pub fn reset_all(&mut self) {
        self.pressed.clear();
        self.just_pressed.clear();
        self.just_released.clear();
    }

    fn clear_just(&mut self) {
        self.just_pressed.clear();
        self.just_released.clear();
    }
}

/// Bevy's `AccumulatedMouseMotion`: raw mouse motion this frame (pixels).
#[derive(Clone, Copy, Debug, Default)]
pub struct AccumulatedMouseMotion {
    pub delta: Vec2,
}

/// Bevy's `AccumulatedMouseScroll`: wheel this frame (lines).
#[derive(Clone, Copy, Debug, Default)]
pub struct AccumulatedMouseScroll {
    pub delta: Vec2,
}

/// Bevy's `GamepadButton` names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GamepadButton {
    South,
    East,
    North,
    West,
    LeftTrigger,
    LeftTrigger2,
    RightTrigger,
    RightTrigger2,
    Select,
    Start,
    Mode,
    LeftThumb,
    RightThumb,
    DPadUp,
    DPadDown,
    DPadLeft,
    DPadRight,
}

/// One gamepad's state (Bevy's `Gamepad` component).
#[derive(Clone, Debug, Default)]
pub struct Gamepad {
    pub(super) buttons: ButtonInput<GamepadButton>,
    pub(super) left: Vec2,
    pub(super) right: Vec2,
}

impl Gamepad {
    pub fn pressed(&self, b: GamepadButton) -> bool { self.buttons.pressed(b) }
    pub fn just_pressed(&self, b: GamepadButton) -> bool { self.buttons.just_pressed(b) }
    /// Y up, as Bevy's.
    pub fn left_stick(&self) -> Vec2 { self.left }
    pub fn right_stick(&self) -> Vec2 { self.right }
    /// Something is held or deflected.
    fn in_use(&self) -> bool { !self.buttons.pressed.is_empty() || self.left != Vec2::ZERO || self.right != Vec2::ZERO }
}

/// Everything the window and the gamepads report, as resources the systems read.
pub struct InputState {
    pub keys: ButtonInput<KeyCode>,
    pub mouse: ButtonInput<MouseButton>,
    pub motion: AccumulatedMouseMotion,
    pub scroll: AccumulatedMouseScroll,
    pub gamepads: Vec<(PadId, Gamepad)>,
    /// The on-screen pad (iOS).
    pub touch: super::touch::TouchPad,
    /// The on-screen pad is the gamepad the game reads (and is drawn): the device used last, touches or a controller
    /// (an idle connected controller, like the Simulator's virtual one, does not take over). iOS starts with it.
    pub touch_active: bool,
    #[cfg(target_os = "macos")]
    gilrs: Option<gilrs::Gilrs>,
}

/// A connected gamepad's identity: gilrs' id, or the GameController object's address (iOS).
#[cfg(target_os = "macos")]
pub type PadId = gilrs::GamepadId;
#[cfg(not(target_os = "macos"))]
pub type PadId = usize;

impl Default for InputState {
    fn default() -> Self {
        InputState {
            keys: ButtonInput::default(),
            mouse: ButtonInput::default(),
            motion: AccumulatedMouseMotion::default(),
            scroll: AccumulatedMouseScroll::default(),
            gamepads: Vec::new(),
            touch: Default::default(),
            touch_active: cfg!(target_os = "ios"),
            #[cfg(target_os = "macos")]
            gilrs: gilrs::Gilrs::new().map_err(|e| eprintln!("gamepads unavailable: {e}")).ok(),
        }
    }
}

impl InputState {
    /// Clears last frame's edges and motion (call before this frame's events are applied).
    pub fn begin_frame(&mut self) {
        self.keys.clear_just();
        self.mouse.clear_just();
        for (_, g) in &mut self.gamepads { g.buttons.clear_just(); }
        self.touch.pad.buttons.clear_just();
        self.motion = AccumulatedMouseMotion::default();
        self.scroll = AccumulatedMouseScroll::default();
    }

    /// The controllers' state for this frame, and which device is in use ([`InputState::touch_active`]).
    pub fn poll_gamepads(&mut self) {
        self.poll_controllers();
        if self.touch.take_touched() {
            self.touch_active = true;
        } else if self.gamepads.iter().any(|(_, g)| g.in_use()) {
            self.touch_active = false;
        }
    }

    /// Drains the gamepad events into the gamepad states.
    #[cfg(target_os = "macos")]
    fn poll_controllers(&mut self) {
        let Some(g) = self.gilrs.as_mut() else { return };
        while let Some(ev) = g.next_event() {
            let idx = match self.gamepads.iter().position(|(id, _)| *id == ev.id) {
                Some(i) => i,
                None => {
                    self.gamepads.push((ev.id, Gamepad::default()));
                    self.gamepads.len() - 1
                }
            };
            let pad = &mut self.gamepads[idx].1;
            use gilrs::{Axis, Button, EventType};
            let map = |b: Button| -> Option<GamepadButton> {
                Some(match b {
                    Button::South => GamepadButton::South,
                    Button::East => GamepadButton::East,
                    Button::North => GamepadButton::North,
                    Button::West => GamepadButton::West,
                    Button::LeftTrigger => GamepadButton::LeftTrigger,
                    Button::LeftTrigger2 => GamepadButton::LeftTrigger2,
                    Button::RightTrigger => GamepadButton::RightTrigger,
                    Button::RightTrigger2 => GamepadButton::RightTrigger2,
                    Button::Select => GamepadButton::Select,
                    Button::Start => GamepadButton::Start,
                    Button::Mode => GamepadButton::Mode,
                    Button::LeftThumb => GamepadButton::LeftThumb,
                    Button::RightThumb => GamepadButton::RightThumb,
                    Button::DPadUp => GamepadButton::DPadUp,
                    Button::DPadDown => GamepadButton::DPadDown,
                    Button::DPadLeft => GamepadButton::DPadLeft,
                    Button::DPadRight => GamepadButton::DPadRight,
                    _ => return None,
                })
            };
            match ev.event {
                EventType::ButtonPressed(b, _) => if let Some(b) = map(b) { pad.buttons.press(b) },
                EventType::ButtonReleased(b, _) => if let Some(b) = map(b) { pad.buttons.release(b) },
                // Bevy's gilrs integration treats the analogue triggers as buttons past 0.5 too.
                EventType::ButtonChanged(b @ (Button::LeftTrigger2 | Button::RightTrigger2), v, _) => {
                    if let Some(b) = map(b) { if v > 0.5 { pad.buttons.press(b) } else { pad.buttons.release(b) } }
                }
                // Bevy's default dead zones: 0.1 on the sticks (its `AxisSettings` default `deadzone_lowerbound/upperbound` ±0.05..0.1).
                EventType::AxisChanged(a, v, _) => {
                    let v = if v.abs() < 0.1 { 0.0 } else { v };
                    match a {
                        Axis::LeftStickX => pad.left.x = v,
                        Axis::LeftStickY => pad.left.y = v,
                        Axis::RightStickX => pad.right.x = v,
                        Axis::RightStickY => pad.right.y = v,
                        _ => {}
                    }
                }
                EventType::Disconnected => {
                    self.gamepads.remove(idx);
                }
                _ => {}
            }
        }
    }

    /// Reads the controllers and the hardware keyboard (GameController keeps their current state; winit reports
    /// no keys on iOS).
    #[cfg(target_os = "ios")]
    fn poll_controllers(&mut self) {
        super::game_controller::poll(self);
    }

    #[cfg(not(any(target_os = "macos", target_os = "ios")))]
    fn poll_controllers(&mut self) {}

    /// The on-screen pad while it is the active device, else the first connected controller.
    pub fn first_gamepad(&self) -> Option<&Gamepad> {
        if self.touch_active { return Some(&self.touch.pad); }
        self.gamepads.first().map(|(_, g)| g)
    }
}
