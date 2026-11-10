//! Minimal GTK key -> terminal byte translation (spike).
//!
//! Plain printable text arrives through the IM context (`commit`), so this only handles special
//! keys and Ctrl/Alt combinations. Later this should be replaced by libghostty-vt's key encoder
//! (`libghostty_vt::key::Encoder`), which also handles the kitty keyboard protocol.

use gtk::gdk::{Key, ModifierType};

/// xterm modifier parameter: 1 + shift(1) + alt(2) + ctrl(4).
fn mod_param(state: ModifierType) -> u8 {
    let mut m = 1;
    if state.contains(ModifierType::SHIFT_MASK) {
        m += 1;
    }
    if state.contains(ModifierType::ALT_MASK) {
        m += 2;
    }
    if state.contains(ModifierType::CONTROL_MASK) {
        m += 4;
    }
    m
}

fn csi_with_mod(final_byte: char, state: ModifierType) -> Vec<u8> {
    match mod_param(state) {
        1 => format!("\x1b[{final_byte}").into_bytes(),
        m => format!("\x1b[1;{m}{final_byte}").into_bytes(),
    }
}

fn tilde(code: u8, state: ModifierType) -> Vec<u8> {
    match mod_param(state) {
        1 => format!("\x1b[{code}~").into_bytes(),
        m => format!("\x1b[{code};{m}~").into_bytes(),
    }
}

pub fn encode_key(keyval: Key, state: ModifierType, app_cursor: bool) -> Option<Vec<u8>> {
    let plain = mod_param(state) == 1;

    // Cursor keys honour DECCKM (application mode) when unmodified.
    let cursor = |c: char| -> Vec<u8> {
        if app_cursor && plain {
            format!("\x1bO{c}").into_bytes()
        } else {
            csi_with_mod(c, state)
        }
    };

    let bytes = match keyval {
        Key::Up | Key::KP_Up => cursor('A'),
        Key::Down | Key::KP_Down => cursor('B'),
        Key::Right | Key::KP_Right => cursor('C'),
        Key::Left | Key::KP_Left => cursor('D'),
        Key::Home | Key::KP_Home => cursor('H'),
        Key::End | Key::KP_End => cursor('F'),
        Key::Page_Up | Key::KP_Page_Up => tilde(5, state),
        Key::Page_Down | Key::KP_Page_Down => tilde(6, state),
        Key::Insert | Key::KP_Insert => tilde(2, state),
        Key::Delete | Key::KP_Delete => tilde(3, state),
        Key::F1 => ss3_or_csi('P', state),
        Key::F2 => ss3_or_csi('Q', state),
        Key::F3 => ss3_or_csi('R', state),
        Key::F4 => ss3_or_csi('S', state),
        Key::F5 => tilde(15, state),
        Key::F6 => tilde(17, state),
        Key::F7 => tilde(18, state),
        Key::F8 => tilde(19, state),
        Key::F9 => tilde(20, state),
        Key::F10 => tilde(21, state),
        Key::F11 => tilde(23, state),
        Key::F12 => tilde(24, state),
        Key::Return | Key::KP_Enter => b"\r".to_vec(),
        Key::BackSpace => {
            if state.contains(ModifierType::CONTROL_MASK) {
                vec![0x08]
            } else {
                vec![0x7f]
            }
        }
        Key::Tab => b"\t".to_vec(),
        Key::ISO_Left_Tab => b"\x1b[Z".to_vec(),
        Key::Escape => vec![0x1b],
        other => return encode_char(other, state),
    };

    Some(bytes)
}

fn ss3_or_csi(c: char, state: ModifierType) -> Vec<u8> {
    match mod_param(state) {
        1 => format!("\x1bO{c}").into_bytes(),
        m => format!("\x1b[1;{m}{c}").into_bytes(),
    }
}

/// Ctrl/Alt + printable key. Anything without Ctrl/Alt is left to the IM context.
fn encode_char(keyval: Key, state: ModifierType) -> Option<Vec<u8>> {
    let ctrl = state.contains(ModifierType::CONTROL_MASK);
    let alt = state.contains(ModifierType::ALT_MASK);
    if !ctrl && !alt {
        return None;
    }
    let ch = keyval.to_unicode()?;

    let mut out = Vec::new();
    if alt {
        out.push(0x1b);
    }
    if ctrl {
        let lower = ch.to_ascii_lowercase();
        let code = match lower {
            'a'..='z' => lower as u8 - b'a' + 1,
            ' ' | '@' | '2' => 0,
            '[' | '3' => 0x1b,
            '\\' | '4' => 0x1c,
            ']' | '5' => 0x1d,
            '^' | '6' => 0x1e,
            '_' | '7' | '/' => 0x1f,
            '8' => 0x7f,
            _ => return None,
        };
        out.push(code);
    } else {
        let mut buf = [0u8; 4];
        out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
    }
    Some(out)
}
