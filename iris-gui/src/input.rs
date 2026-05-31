//! Translate egui keyboard / pointer events into iris PS2 controller writes.
//!
//! The framebuffer panel calls `pump(...)` each frame with the rect the
//! REX3 image occupies in screen space. We then:
//!
//! 1. Track previous modifier state and synthesise ShiftLeft / ControlLeft
//!    / AltLeft / SuperLeft press / release events as needed (egui delivers
//!    modifiers as a separate field, not as Key events).
//! 2. Forward every egui::Event::Key whose `Key` we can map to a winit
//!    `KeyCode`, in the order egui produced them.
//! 3. Build PS/2 mouse packets from button state changes and cursor
//!    motion, but only when the cursor is inside the framebuffer rect
//!    (so menu / config clicks don't leak into the guest).

use egui::{Event, Key, Modifiers, PointerButton, Rect};
use iris::ps2::Ps2Controller;
use winit::keyboard::KeyCode;

pub struct InputState {
    last_mods: Modifiers,
    last_buttons: u8,         // bit0=L, bit1=R, bit2=M
    last_pos: Option<egui::Pos2>,
}

impl Default for InputState {
    fn default() -> Self {
        Self { last_mods: Modifiers::NONE, last_buttons: 0, last_pos: None }
    }
}

pub fn pump(ctx: &egui::Context, fb_rect: Rect, ps2: &Ps2Controller, state: &mut InputState) {
    ctx.input(|i| {
        // ---- modifiers: diff previous → current, synth press/release. ----
        let m = i.modifiers;
        if m.shift && !state.last_mods.shift { ps2.push_kb(KeyCode::ShiftLeft, true); }
        if !m.shift && state.last_mods.shift { ps2.push_kb(KeyCode::ShiftLeft, false); }
        if m.ctrl  && !state.last_mods.ctrl  { ps2.push_kb(KeyCode::ControlLeft, true); }
        if !m.ctrl  && state.last_mods.ctrl  { ps2.push_kb(KeyCode::ControlLeft, false); }
        if m.alt   && !state.last_mods.alt   { ps2.push_kb(KeyCode::AltLeft, true); }
        if !m.alt   && state.last_mods.alt   { ps2.push_kb(KeyCode::AltLeft, false); }
        if m.mac_cmd && !state.last_mods.mac_cmd { ps2.push_kb(KeyCode::SuperLeft, true); }
        if !m.mac_cmd && state.last_mods.mac_cmd { ps2.push_kb(KeyCode::SuperLeft, false); }
        state.last_mods = m;

        // ---- key events ----
        for ev in &i.events {
            if let Event::Key { key, pressed, repeat: _, modifiers: _, .. } = ev {
                if let Some(kc) = map_key(*key) {
                    ps2.push_kb(kc, *pressed);
                }
            }
        }

        // ---- mouse: only when the pointer is over the framebuffer ----
        let Some(pos) = i.pointer.latest_pos() else { return; };
        if !fb_rect.contains(pos) {
            state.last_pos = None;
            return;
        }

        // Button state diff.
        let mut buttons = 0u8;
        if i.pointer.button_down(PointerButton::Primary)   { buttons |= 0x01; }
        if i.pointer.button_down(PointerButton::Secondary) { buttons |= 0x02; }
        if i.pointer.button_down(PointerButton::Middle)    { buttons |= 0x04; }

        // Motion: skip the very first sample (no anchor), then send deltas.
        let (dx, dy) = match state.last_pos {
            Some(prev) => ((pos.x - prev.x) as i32, (pos.y - prev.y) as i32),
            None => (0, 0),
        };
        state.last_pos = Some(pos);

        if buttons != state.last_buttons || dx != 0 || dy != 0 {
            send_mouse_packet(ps2, buttons, dx, -dy); // PS/2 Y axis is up-positive
            state.last_buttons = buttons;
        }
    });
}

/// Build and dispatch one PS/2 mouse packet. Mirrors `src/ui.rs:646–658`:
/// byte 0 bit3 always 1, bits 2..0 are buttons (M/R/L), bit 4 = X sign,
/// bit 5 = Y sign, bits 6/7 = X/Y overflow.
fn send_mouse_packet(ps2: &Ps2Controller, buttons: u8, dx: i32, dy: i32) {
    // Clamp to the 9-bit signed range expected by the protocol. Real
    // drivers split large moves; that's fine to skip here because egui
    // delivers small per-frame deltas.
    let sx = dx.clamp(-256, 255);
    let sy = dy.clamp(-256, 255);
    let mut b0 = 0x08 | (buttons & 0x07);
    if sx < 0 { b0 |= 0x10; }
    if sy < 0 { b0 |= 0x20; }
    if sx < -256 || sx > 255 { b0 |= 0x40; }
    if sy < -256 || sy > 255 { b0 |= 0x80; }
    ps2.push_mouse_packet(b0, sx as u8, sy as u8);
}

/// egui::Key → winit::keyboard::KeyCode. Returns None for keys iris's
/// scancode mapper doesn't recognise (we just drop them rather than
/// inventing a fallback).
fn map_key(k: Key) -> Option<KeyCode> {
    Some(match k {
        // Letters
        Key::A => KeyCode::KeyA, Key::B => KeyCode::KeyB, Key::C => KeyCode::KeyC,
        Key::D => KeyCode::KeyD, Key::E => KeyCode::KeyE, Key::F => KeyCode::KeyF,
        Key::G => KeyCode::KeyG, Key::H => KeyCode::KeyH, Key::I => KeyCode::KeyI,
        Key::J => KeyCode::KeyJ, Key::K => KeyCode::KeyK, Key::L => KeyCode::KeyL,
        Key::M => KeyCode::KeyM, Key::N => KeyCode::KeyN, Key::O => KeyCode::KeyO,
        Key::P => KeyCode::KeyP, Key::Q => KeyCode::KeyQ, Key::R => KeyCode::KeyR,
        Key::S => KeyCode::KeyS, Key::T => KeyCode::KeyT, Key::U => KeyCode::KeyU,
        Key::V => KeyCode::KeyV, Key::W => KeyCode::KeyW, Key::X => KeyCode::KeyX,
        Key::Y => KeyCode::KeyY, Key::Z => KeyCode::KeyZ,
        // Digits
        Key::Num0 => KeyCode::Digit0, Key::Num1 => KeyCode::Digit1,
        Key::Num2 => KeyCode::Digit2, Key::Num3 => KeyCode::Digit3,
        Key::Num4 => KeyCode::Digit4, Key::Num5 => KeyCode::Digit5,
        Key::Num6 => KeyCode::Digit6, Key::Num7 => KeyCode::Digit7,
        Key::Num8 => KeyCode::Digit8, Key::Num9 => KeyCode::Digit9,
        // Navigation / editing
        Key::Escape    => KeyCode::Escape,
        Key::Tab       => KeyCode::Tab,
        Key::Backspace => KeyCode::Backspace,
        Key::Enter     => KeyCode::Enter,
        Key::Space     => KeyCode::Space,
        Key::Insert    => KeyCode::Insert,
        Key::Delete    => KeyCode::Delete,
        Key::Home      => KeyCode::Home,
        Key::End       => KeyCode::End,
        Key::PageUp    => KeyCode::PageUp,
        Key::PageDown  => KeyCode::PageDown,
        Key::ArrowUp    => KeyCode::ArrowUp,
        Key::ArrowDown  => KeyCode::ArrowDown,
        Key::ArrowLeft  => KeyCode::ArrowLeft,
        Key::ArrowRight => KeyCode::ArrowRight,
        // Punctuation
        Key::Comma        => KeyCode::Comma,
        Key::Period       => KeyCode::Period,
        Key::Slash        => KeyCode::Slash,
        Key::Backslash    => KeyCode::Backslash,
        Key::Minus        => KeyCode::Minus,
        Key::Equals       => KeyCode::Equal,
        Key::Plus         => KeyCode::Equal, // shifted: same physical key
        Key::Semicolon    => KeyCode::Semicolon,
        Key::Colon        => KeyCode::Semicolon,
        Key::Quote        => KeyCode::Quote,
        Key::OpenBracket  => KeyCode::BracketLeft,
        Key::CloseBracket => KeyCode::BracketRight,
        Key::Backtick     => KeyCode::Backquote,
        // F-keys (egui has no F5; iris likely doesn't need F13+ either)
        Key::F1 => KeyCode::F1, Key::F2 => KeyCode::F2,  Key::F3  => KeyCode::F3,
        Key::F4 => KeyCode::F4, Key::F6 => KeyCode::F6,  Key::F7  => KeyCode::F7,
        Key::F8 => KeyCode::F8, Key::F9 => KeyCode::F9,  Key::F10 => KeyCode::F10,
        // F11 is consumed by the GUI (fullscreen toggle); don't forward.
        Key::F12 => KeyCode::F12,
        _ => return None,
    })
}
