//! Catppuccin Mocha. Swapping flavour means changing only this file.

// The whole palette lives here; later milestones use the rest.
#![allow(dead_code)]

use ratatui::style::Color;

pub const BASE: Color = Color::Rgb(0x1e, 0x1e, 0x2e);
pub const MANTLE: Color = Color::Rgb(0x18, 0x18, 0x25);
pub const CRUST: Color = Color::Rgb(0x11, 0x11, 0x1b);
pub const SURFACE0: Color = Color::Rgb(0x31, 0x32, 0x44);
pub const SURFACE1: Color = Color::Rgb(0x45, 0x47, 0x5a);
pub const OVERLAY0: Color = Color::Rgb(0x6c, 0x70, 0x86);
pub const SUBTEXT0: Color = Color::Rgb(0xa6, 0xad, 0xc8);
pub const TEXT: Color = Color::Rgb(0xcd, 0xd6, 0xf4);
pub const LAVENDER: Color = Color::Rgb(0xb4, 0xbe, 0xfe);
pub const BLUE: Color = Color::Rgb(0x89, 0xb4, 0xfa);
pub const MAUVE: Color = Color::Rgb(0xcb, 0xa6, 0xf7);
pub const YELLOW: Color = Color::Rgb(0xf9, 0xe2, 0xaf);
pub const TEAL: Color = Color::Rgb(0x94, 0xe2, 0xd5);
pub const PEACH: Color = Color::Rgb(0xfa, 0xb3, 0x87);
pub const GREEN: Color = Color::Rgb(0xa6, 0xe3, 0xa1);
pub const RED: Color = Color::Rgb(0xf3, 0x8b, 0xa8);

/// Popups: a crisp white border over the app's own background, and text
/// that stays bright; the dim tone is for hints and footers.
pub const POPUP_BORDER: Color = TEXT;
pub const POPUP_BG: Color = BASE;
pub const POPUP_DIM: Color = SUBTEXT0;

/// Cursor row, focused header, mode chip.
pub const ACCENT: Color = LAVENDER;
/// Text on an accent background.
pub const ON_ACCENT: Color = CRUST;
