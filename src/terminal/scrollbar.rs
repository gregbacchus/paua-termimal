//! Scrollbar and scroll-to-bottom button: pure geometry (used for drawing and hit-testing).
//!
//! Both are drawn on top of the terminal, at its right edge: a thin thumb that fades in while
//! scrolling or when the pointer is near, and a small round button that appears while the view is
//! scrolled away from the bottom.

use serde::Deserialize;

use crate::config::Tint;

/// When the scrollbar is shown (it only ever exists while there is scrollback).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ScrollbarMode {
    /// Fades in while scrolling or when the pointer is at the right edge, fades out when idle.
    #[default]
    Auto,
    /// Always visible while there is scrollback.
    Always,
    Never,
}

/// How far the viewport is scrolled, in rows (fractional while smooth-scrolling).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Metrics {
    pub total: f32,
    pub offset: f32,
    pub len: f32,
}

impl Metrics {
    pub fn scrollable(&self) -> bool {
        self.total > self.len + 0.5
    }

    pub fn at_bottom(&self) -> bool {
        self.offset + self.len >= self.total - 0.01
    }
}

/// Look and timing of the scrollbar.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScrollbarStyle {
    pub mode: ScrollbarMode,
    /// Thumb width at rest and while the pointer is near, in px.
    pub width: f32,
    pub hover_width: f32,
    /// Gap between the thumb and the pane's right edge, in px.
    pub margin: f32,
    pub min_thumb: f32,
    /// Thumb colour; `None` = the pane's text colour.
    pub color: Option<Tint>,
    /// Opacity at rest / while hovered / of the track shown while hovered (multiplied with the
    /// colour's own alpha and the fade).
    pub opacity: f32,
    pub hover_opacity: f32,
    pub track_opacity: f32,
    /// How long it stays visible after scrolling (`auto` mode) and how long fades take.
    pub idle_ms: u32,
    pub fade_ms: u32,
}

impl Default for ScrollbarStyle {
    fn default() -> Self {
        Self {
            mode: ScrollbarMode::Auto,
            width: 4.0,
            hover_width: 8.0,
            margin: 3.0,
            min_thumb: 28.0,
            color: None,
            opacity: 0.32,
            hover_opacity: 0.55,
            track_opacity: 0.08,
            idle_ms: 1200,
            fade_ms: 200,
        }
    }
}

/// Look of the scroll-to-bottom button.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ButtonStyle {
    pub enabled: bool,
    pub size: f32,
    pub margin_right: f32,
    pub margin_bottom: f32,
    /// Circle and chevron colour; `None` = the pane's text colour.
    pub color: Option<Tint>,
    pub bg_opacity: f32,
    pub bg_hover_opacity: f32,
    pub icon_opacity: f32,
    pub icon_hover_opacity: f32,
}

impl Default for ButtonStyle {
    fn default() -> Self {
        Self {
            enabled: true,
            size: 28.0,
            margin_right: 20.0,
            margin_bottom: 14.0,
            color: None,
            bg_opacity: 0.14,
            bg_hover_opacity: 0.28,
            icon_opacity: 0.7,
            icon_hover_opacity: 1.0,
        }
    }
}

/// Top/bottom gap of the scrollbar track.
const TRACK_MARGIN: f32 = 4.0;

/// Width of the strip along the right edge that reacts to the pointer.
pub fn zone_w(style: &ScrollbarStyle) -> f32 {
    (style.hover_width + style.margin + 3.0).max(14.0)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Geo {
    pub track_top: f32,
    pub track_h: f32,
    pub thumb_y: f32,
    pub thumb_h: f32,
}

/// Track and thumb for a widget of height `h`. `None` if there is nothing to scroll.
pub fn geometry(h: f32, m: Metrics, style: &ScrollbarStyle) -> Option<Geo> {
    if !m.scrollable() {
        return None;
    }
    let track_top = TRACK_MARGIN;
    let track_h = (h - 2.0 * TRACK_MARGIN).max(1.0);
    let thumb_h = (track_h * m.len / m.total).clamp(style.min_thumb.min(track_h), track_h);
    let range = m.total - m.len;
    let thumb_y = track_top + (track_h - thumb_h) * (m.offset / range).clamp(0.0, 1.0);
    Some(Geo { track_top, track_h, thumb_y, thumb_h })
}

/// The row offset that puts the thumb's top at `thumb_top` (for dragging and track clicks).
pub fn offset_for_thumb_top(g: &Geo, m: Metrics, thumb_top: f32) -> f32 {
    let free = (g.track_h - g.thumb_h).max(1.0);
    ((thumb_top - g.track_top) / free).clamp(0.0, 1.0) * (m.total - m.len)
}

/// Top-left of the scroll-to-bottom button for a widget of size `w` x `h`.
pub fn button_origin(w: f32, h: f32, b: &ButtonStyle) -> (f32, f32) {
    (w - b.margin_right - b.size, h - b.margin_bottom - b.size)
}

pub fn in_button(w: f32, h: f32, x: f32, y: f32, b: &ButtonStyle) -> bool {
    let (bx, by) = button_origin(w, h, b);
    x >= bx && x <= bx + b.size && y >= by && y <= by + b.size
}

pub fn in_zone(w: f32, x: f32, style: &ScrollbarStyle) -> bool {
    x >= w - zone_w(style)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(total: f32, offset: f32, len: f32) -> Metrics {
        Metrics { total, offset, len }
    }

    #[test]
    fn nothing_to_scroll_means_no_scrollbar() {
        let st = ScrollbarStyle::default();
        assert!(geometry(400.0, m(24.0, 0.0, 24.0), &st).is_none());
        assert!(geometry(400.0, m(24.4, 0.0, 24.0), &st).is_none());
    }

    #[test]
    fn thumb_size_and_position_follow_the_viewport() {
        let st = ScrollbarStyle::default();
        let top = geometry(408.0, m(100.0, 0.0, 25.0), &st).unwrap();
        assert!((top.thumb_y - top.track_top).abs() < 1e-3);
        assert!((top.thumb_h - 100.0).abs() < 1e-3); // 25% of a 400px track

        let bottom = geometry(408.0, m(100.0, 75.0, 25.0), &st).unwrap();
        assert!((bottom.thumb_y + bottom.thumb_h - (bottom.track_top + bottom.track_h)).abs() < 1e-3);

        // A huge scrollback keeps a grabbable minimum thumb, which is configurable.
        let tiny = geometry(408.0, m(100_000.0, 0.0, 25.0), &st).unwrap();
        assert!((tiny.thumb_h - 28.0).abs() < 1e-3);
        let bigger = ScrollbarStyle { min_thumb: 50.0, ..st };
        assert!((geometry(408.0, m(100_000.0, 0.0, 25.0), &bigger).unwrap().thumb_h - 50.0).abs() < 1e-3);
    }

    #[test]
    fn dragging_maps_back_to_rows() {
        let st = ScrollbarStyle::default();
        let met = m(100.0, 30.0, 25.0);
        let g = geometry(408.0, met, &st).unwrap();
        // The thumb's current top maps back to the current offset.
        assert!((offset_for_thumb_top(&g, met, g.thumb_y) - 30.0).abs() < 1e-2);
        // Beyond either end clamps.
        assert_eq!(offset_for_thumb_top(&g, met, -50.0), 0.0);
        assert_eq!(offset_for_thumb_top(&g, met, 10_000.0), 75.0);
    }

    #[test]
    fn bottom_detection_and_hit_zones_follow_the_style() {
        let (st, btn) = (ScrollbarStyle::default(), ButtonStyle::default());
        assert!(m(100.0, 75.0, 25.0).at_bottom());
        assert!(!m(100.0, 74.0, 25.0).at_bottom());
        assert!(in_zone(500.0, 490.0, &st) && !in_zone(500.0, 400.0, &st));
        // A wider hover thumb widens the zone.
        let wide = ScrollbarStyle { hover_width: 20.0, ..st };
        assert!(zone_w(&wide) > zone_w(&st) && in_zone(500.0, 480.0, &wide) && !in_zone(500.0, 480.0, &st));

        let (bx, by) = button_origin(500.0, 400.0, &btn);
        assert!(in_button(500.0, 400.0, bx + 5.0, by + 5.0, &btn));
        assert!(!in_button(500.0, 400.0, bx - 5.0, by + 5.0, &btn));
        let big = ButtonStyle { size: 40.0, ..btn };
        assert!(in_button(500.0, 400.0, 500.0 - 20.0 - 39.0, 400.0 - 14.0 - 39.0, &big));
    }
}
