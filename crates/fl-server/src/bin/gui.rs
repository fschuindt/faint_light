//! `faint-light-gui` — the desktop front end.
//!
//! A separate binary from `faint-light` because a Windows GUI must not be
//! linked into the console subsystem (it would open a console window behind
//! the app), and because the headless server should stay free of the
//! toolkit entirely.

// No console window on Windows; the Logs tab is where the output goes.
#![cfg_attr(windows, windows_subsystem = "windows")]

use tracing_subscriber::EnvFilter;

use fl_server::gui::{self, LogBuffer};

fn main() {
    // The Logs tab is the destination; on Windows the GUI subsystem has no
    // stderr, so there is nowhere else for it to go. The file behind it is
    // opened first, so this session's very first line is kept too.
    let buffer = LogBuffer::new();
    buffer.persist_to(gui::log_path());
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_ansi(false)
        .with_writer(buffer.clone())
        .init();

    tracing::info!(version = fl_server::version(), "faint_light GUI starting");
    if let Err(e) = gui::run(buffer) {
        eprintln!("faint_light: {e}");
        fltk::dialog::alert_default(&format!("Faint Light could not start.\n\n{e}"));
        std::process::exit(1);
    }
}
