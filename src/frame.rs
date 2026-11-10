//! The window frame colour shared by the chrome and the active tab underline.
//!
//! The colour is arbitrary (from config), but GTK only animates CSS property changes between
//! *computed values*. So we keep two CSS classes, `frame-a` and `frame-b`. To change colour we
//! write the new colour into whichever slot is NOT active (nothing visibly changes), then flip the
//! window's class to that slot; the `transition` rules in style.css do the fade.

use std::cell::{Cell, RefCell};

use gtk::{gdk, prelude::*};

use crate::config::Tint;
use crate::terminal::Rgb;

#[derive(Clone, Copy, PartialEq)]
pub struct TabUnderline {
    pub height: u8,
    pub color: Option<Tint>,
}

pub struct Frame {
    /// CSS class names for the two slots, unique per window (`frame-<id>-a` / `-b`).
    slots: [String; 2],
    provider: gtk::CssProvider,
    css: RefCell<[String; 2]>,
    active: Cell<usize>,
    current: Cell<Option<(Rgb, TabUnderline)>>,
    /// How much darker the frame gets while its window is unfocused (0..1).
    backdrop: Cell<f32>,
}

impl Frame {
    /// One `Frame` per window; `id` keeps the CSS class names of different windows apart.
    pub fn new(id: u32) -> Self {
        let provider = gtk::CssProvider::new();
        gtk::style_context_add_provider_for_display(
            &gdk::Display::default().expect("no display"),
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
        Self {
            slots: [format!("frame-{id}-a"), format!("frame-{id}-b")],
            provider,
            css: RefCell::default(),
            active: Cell::new(0),
            current: Cell::new(None),
            backdrop: Cell::new(0.12),
        }
    }

    /// Change how much the frame darkens when its window loses focus (0 = not at all).
    pub fn set_backdrop(&self, amount: f32) {
        let amount = amount.clamp(0.0, 1.0);
        if (self.backdrop.get() - amount).abs() < 1e-4 {
            return;
        }
        self.backdrop.set(amount);
        if let Some((color, underline)) = self.current.get() {
            // Rewrite the active slot in place (same colour, so nothing animates).
            let slot = self.active.get();
            self.css.borrow_mut()[slot] = rule(&self.slots[slot], color, amount, underline);
            self.provider.load_from_string(&self.css.borrow().join("\n"));
        }
    }

    /// Set the frame colour (animated if it isn't the first call).
    pub fn set(&self, window: &impl IsA<gtk::Widget>, color: Rgb, underline: TabUnderline) {
        if self.current.get() == Some((color, underline)) {
            return;
        }
        let first = self.current.get().is_none();
        let old = self.active.get();
        let new = if first { old } else { 1 - old };

        self.css.borrow_mut()[new] = rule(&self.slots[new], color, self.backdrop.get(), underline);
        self.provider.load_from_string(&self.css.borrow().join("\n"));

        if !first {
            window.remove_css_class(&self.slots[old]);
        }
        window.add_css_class(&self.slots[new]);
        self.active.set(new);
        self.current.set(Some((color, underline)));
    }
}

fn rule(slot: &str, c: Rgb, backdrop_amount: f32, underline: TabUnderline) -> String {
    // Readable text on whatever colour we were given.
    let fg = if c.luminance() > 0.6 { Rgb(0x1b, 0x1b, 0x1f) } else { Rgb(0xff, 0xff, 0xff) };
    let backdrop = c.mix(Rgb(0, 0, 0), backdrop_amount); // dimmer while the window is unfocused
    let mut css = format!(
        "window.{slot} {{ background-color: {bg}; }}\n\
         window.{slot}:backdrop {{ background-color: {bd}; }}\n\
         window.{slot} headerbar, window.{slot} .status-bar {{ color: {fg}; }}\n\
         window.{slot} .command-center {{ background-color: rgba({r},{g},{b},0.16); }}\n",
        bg = c.css(),
        bd = backdrop.css(),
        fg = fg.css(),
        r = fg.0,
        g = fg.1,
        b = fg.2,
    );
    if underline.height > 0 {
        let selector = ".pane-header-row.multiple-tabs .pane-header.selected";
        let shadow = |color: &str| {
            format!("box-shadow: inset -1px 0 rgba(128, 128, 128, 0.25), inset 0 -{}px {color};", underline.height)
        };
        let active = underline.color.map_or_else(|| c.css(), |tint| tint.css());
        css += &format!("window.themed.{slot} {selector} {{ {} }}\n", shadow(&active));
        if underline.color.is_none() {
            css += &format!("window.themed.{slot}:backdrop {selector} {{ {} }}\n", shadow(&backdrop.css()));
        }
    }
    css
}
