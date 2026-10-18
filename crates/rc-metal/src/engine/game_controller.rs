//! iOS input through Apple's GameController framework: the connected controllers (extended gamepad profile: Xbox,
//! PlayStation, MFi) and the hardware keyboard (an iPad's, or the Mac's in the simulator), polled once per frame into
//! [`InputState`] with the same names as the macOS path (gilrs, winit keys).

use super::input::{Gamepad, GamepadButton, InputState, KeyCode};
use glam::Vec2;
use objc2_game_controller::*;

/// The keys the game reads (crate::input_map, crate::fly_cam, the debug keys) and their GameController codes.
fn keys() -> Vec<(GCKeyCode, KeyCode)> {
    // SAFETY: the GameController key-code constants are plain immutable values.
    unsafe {
        vec![
            (GCKeyCodeKeyA, KeyCode::KeyA), (GCKeyCodeKeyC, KeyCode::KeyC), (GCKeyCodeKeyD, KeyCode::KeyD),
            (GCKeyCodeKeyE, KeyCode::KeyE), (GCKeyCodeKeyF, KeyCode::KeyF), (GCKeyCodeKeyG, KeyCode::KeyG),
            (GCKeyCodeKeyH, KeyCode::KeyH), (GCKeyCodeKeyJ, KeyCode::KeyJ), (GCKeyCodeKeyK, KeyCode::KeyK),
            (GCKeyCodeKeyL, KeyCode::KeyL), (GCKeyCodeKeyN, KeyCode::KeyN), (GCKeyCodeKeyP, KeyCode::KeyP),
            (GCKeyCodeKeyQ, KeyCode::KeyQ), (GCKeyCodeKeyR, KeyCode::KeyR), (GCKeyCodeKeyS, KeyCode::KeyS),
            (GCKeyCodeKeyT, KeyCode::KeyT), (GCKeyCodeKeyV, KeyCode::KeyV), (GCKeyCodeKeyW, KeyCode::KeyW),
            (GCKeyCodeKeyZ, KeyCode::KeyZ), (GCKeyCodeComma, KeyCode::Comma), (GCKeyCodePeriod, KeyCode::Period),
            (GCKeyCodeSpacebar, KeyCode::Space), (GCKeyCodeReturnOrEnter, KeyCode::Enter),
            (GCKeyCodeDeleteOrBackspace, KeyCode::Backspace), (GCKeyCodeEscape, KeyCode::Escape),
            (GCKeyCodeTab, KeyCode::Tab), (GCKeyCodeLeftShift, KeyCode::ShiftLeft),
            (GCKeyCodeRightShift, KeyCode::ShiftRight), (GCKeyCodeLeftControl, KeyCode::ControlLeft),
            (GCKeyCodeUpArrow, KeyCode::ArrowUp), (GCKeyCodeDownArrow, KeyCode::ArrowDown),
            (GCKeyCodeLeftArrow, KeyCode::ArrowLeft), (GCKeyCodeRightArrow, KeyCode::ArrowRight),
            (GCKeyCodeF9, KeyCode::F9), (GCKeyCodeF10, KeyCode::F10), (GCKeyCodeF11, KeyCode::F11),
        ]
    }
}

/// The controllers' and the keyboard's current state into `input`.
pub fn poll(input: &mut InputState) {
    // SAFETY: GameController's class and instance getters, called on the main thread (the winit loop's).
    unsafe {
        if let Some(kb) = GCKeyboard::coalescedKeyboard().and_then(|k| k.keyboardInput()) {
            for (code, key) in keys() {
                let down = kb.buttonForKeyCode(code).is_some_and(|b| b.isPressed());
                if down { input.keys.press(key) } else { input.keys.release(key) }
            }
        }
        let controllers = GCController::controllers();
        let mut seen = Vec::new();
        for c in controllers.iter() {
            let Some(g) = c.extendedGamepad() else { continue };
            let id = objc2::rc::Retained::as_ptr(&c) as usize;
            seen.push(id);
            let i = match input.gamepads.iter().position(|(p, _)| *p == id) {
                Some(i) => i,
                None => {
                    let name = c.vendorName().map_or("?".to_string(), |n| n.to_string());
                    println!("input: controller connected: {name} (the on-screen pad hides once it is used)");
                    input.gamepads.push((id, Gamepad::default()));
                    input.gamepads.len() - 1
                }
            };
            let pad = &mut input.gamepads[i].1;
            let opt = |b: Option<objc2::rc::Retained<GCControllerButtonInput>>| b.is_some_and(|b| b.isPressed());
            let dpad = g.dpad();
            let state = [
                (GamepadButton::South, g.buttonA().isPressed()),
                (GamepadButton::East, g.buttonB().isPressed()),
                (GamepadButton::West, g.buttonX().isPressed()),
                (GamepadButton::North, g.buttonY().isPressed()),
                (GamepadButton::LeftTrigger, g.leftShoulder().isPressed()),
                (GamepadButton::RightTrigger, g.rightShoulder().isPressed()),
                // As the gilrs path: the analogue triggers count past 0.5.
                (GamepadButton::LeftTrigger2, g.leftTrigger().value() > 0.5),
                (GamepadButton::RightTrigger2, g.rightTrigger().value() > 0.5),
                (GamepadButton::Start, g.buttonMenu().isPressed()),
                (GamepadButton::Select, opt(g.buttonOptions())),
                (GamepadButton::Mode, opt(g.buttonHome())),
                (GamepadButton::LeftThumb, opt(g.leftThumbstickButton())),
                (GamepadButton::RightThumb, opt(g.rightThumbstickButton())),
                (GamepadButton::DPadUp, dpad.up().isPressed()),
                (GamepadButton::DPadDown, dpad.down().isPressed()),
                (GamepadButton::DPadLeft, dpad.left().isPressed()),
                (GamepadButton::DPadRight, dpad.right().isPressed()),
            ];
            for (b, down) in state {
                if down { pad.buttons.press(b) } else { pad.buttons.release(b) }
            }
            // Y up, as gilrs'; the same dead zone.
            let stick = |s: objc2::rc::Retained<GCControllerDirectionPad>| {
                let v = Vec2::new(s.xAxis().value(), s.yAxis().value());
                Vec2::new(if v.x.abs() < 0.1 { 0.0 } else { v.x }, if v.y.abs() < 0.1 { 0.0 } else { v.y })
            };
            pad.left = stick(g.leftThumbstick());
            pad.right = stick(g.rightThumbstick());
        }
        let before = input.gamepads.len();
        input.gamepads.retain(|(p, _)| seen.contains(p));
        if input.gamepads.len() < before { println!("input: controller disconnected ({} left)", input.gamepads.len()); }
    }
}
