//! The classic Windows 2000 look, applied on every platform.
//!
//! FLTK's `Base` scheme already draws the Windows 95/2000 bevels, so this is
//! mostly a palette and a font: the "3D Objects" grey, the navy selection,
//! Tahoma at 8pt. Pinning them rather than inheriting the host theme means
//! the window looks the same on Windows, on Linux, and on the ARM builds to
//! come.

use fltk::app;
use fltk::button::Button;
use fltk::enums::{Align, Color, Font, FrameType};
use fltk::frame::Frame;
use fltk::input::Input;
use fltk::output::Output;
use fltk::prelude::*;

/// Windows 2000 "3D Objects" grey.
pub const FACE: Color = Color::from_rgb(212, 208, 200);
/// Text-entry white.
pub const FIELD: Color = Color::from_rgb(255, 255, 255);
/// Greyed-out label text, for hints.
pub const DIM: Color = Color::from_rgb(128, 128, 128);
/// Text that has to be read rather than skimmed -- the client URLs.
pub const NOTE: Color = Color::from_rgb(0, 0, 0);
/// The green/red of the server status dot.
pub const RUNNING: Color = Color::from_rgb(0, 128, 0);
pub const STOPPED: Color = Color::from_rgb(128, 0, 0);

/// Standard control metrics, in pixels — everything is laid out in these so
/// the proportions stay classic rather than drifting per widget.
pub const ROW: i32 = 22;
pub const GAP: i32 = 6;
pub const PAD: i32 = 10;
pub const LABEL_W: i32 = 118;
pub const BUTTON_W: i32 = 80;
/// Controls and labels: Tahoma 8pt, as Windows 2000 drew them.
pub const FONT_SIZE: i32 = 11;
/// The log view. Bigger, because it is prose to read, not a form to fill.
pub const LOG_FONT_SIZE: i32 = 13;

pub fn apply() {
    app::set_scheme(app::Scheme::Base);
    app::background(212, 208, 200);
    app::background2(255, 255, 255);
    app::foreground(0, 0, 0);
    // Windows 2000 "Selected Items" navy.
    app::set_selection_color(10, 36, 106);
    app::set_visible_focus(true);
    // Tahoma is the Windows 2000 shell font; elsewhere FLTK falls back to
    // whatever the platform offers for Helvetica, which is the right
    // behaviour — a missing face should not mean an unreadable window.
    Font::set_font(Font::Helvetica, "Tahoma");
    Font::set_font(Font::HelveticaBold, "Tahoma Bold");
    // Courier New is unpleasant to read a log in. Consolas ships with
    // Windows; elsewhere ask for the generic monospace family.
    Font::set_font(
        Font::Courier,
        if cfg!(windows) {
            "Consolas"
        } else {
            "monospace"
        },
    );
    app::set_font_size(FONT_SIZE);
}

/// A left-aligned form label, the way a Windows dialog labels a field.
pub fn label(x: i32, y: i32, w: i32, text: &str) -> Frame {
    let mut f = Frame::new(x, y, w, ROW, None).with_label(text);
    f.set_align(Align::Inside | Align::Left);
    f.set_label_color(Color::Black);
    f
}

/// Small dim text under a control.
pub fn note(x: i32, y: i32, w: i32, h: i32, text: &str) -> Frame {
    let mut f = Frame::new(x, y, w, h, None).with_label(text);
    f.set_align(Align::Inside | Align::Left | Align::Top | Align::Wrap);
    f.set_label_color(DIM);
    f
}

/// A section heading: bold text over a horizontal rule.
pub fn heading(x: i32, y: i32, w: i32, text: &str) -> Frame {
    let mut f = Frame::new(x, y, w, ROW, None).with_label(text);
    f.set_align(Align::Inside | Align::Left | Align::Bottom);
    f.set_label_font(Font::HelveticaBold);
    f
}

/// The engraved horizontal rule under a heading.
pub fn rule(x: i32, y: i32, w: i32) -> Frame {
    let mut f = Frame::new(x, y, w, 2, None);
    f.set_frame(FrameType::EngravedFrame);
    f
}

pub fn input(x: i32, y: i32, w: i32) -> Input {
    let mut i = Input::new(x, y, w, ROW, None);
    i.set_frame(FrameType::DownBox);
    i.set_color(FIELD);
    i.set_text_size(FONT_SIZE);
    i
}

/// A read-only field. Windows 2000 shows these sunken but grey, which is
/// exactly what distinguishes "the program computed this" from "type here".
pub fn output(x: i32, y: i32, w: i32) -> Output {
    let mut o = Output::new(x, y, w, ROW, None);
    o.set_frame(FrameType::DownBox);
    o.set_color(FACE);
    o.set_text_size(FONT_SIZE);
    o
}

pub fn button(x: i32, y: i32, w: i32, text: &str) -> Button {
    let mut b = Button::new(x, y, w, ROW + 2, None).with_label(text);
    b.set_frame(FrameType::UpBox);
    b.set_down_frame(FrameType::DownBox);
    b
}
