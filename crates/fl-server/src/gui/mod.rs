//! The optional FLTK desktop front end (`--features gui`).
//!
//! A small window over one embedded server: a Server tab that configures and
//! runs the listener, and a Logs tab that mirrors the tracing output the
//! headless binary prints. Solving belongs to the API — and, later, to the
//! web UI the server will serve at its root.

mod autostart;
pub mod logs;
mod runner;
mod settings;
mod theme;
mod tray;

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;

use fltk::app;
use fltk::button::{Button, CheckButton};
use fltk::dialog;
use fltk::enums::{Color, Font, FrameType};
use fltk::frame::Frame;
use fltk::group::{Group, Tabs};
use fltk::image::{PngImage, RgbImage};
use fltk::input::Input;
use fltk::output::Output;
use fltk::prelude::*;
use fltk::text::{TextBuffer, TextDisplay, WrapMode};
use fltk::window::Window;

pub use logs::LogBuffer;
use runner::Runner;
use settings::Settings;
use theme::{BUTTON_W, GAP, LABEL_W, PAD, ROW};

const ICON_PNG: &[u8] = include_bytes!("../../../../assets/faint-light-eye.png");

/// Where the GUI keeps its log, beside the settings file. Call
/// [`LogBuffer::persist_to`] with this before installing the subscriber, so
/// the first line of the session is recorded too.
pub fn log_path() -> std::path::PathBuf {
    settings::config_dir()
        .join("faint_light")
        .join("faint-light.log")
}

const WIN_W: i32 = 620;
const WIN_H: i32 = 448;
const TAB_X: i32 = 8;
const TAB_Y: i32 = 8;
const TAB_W: i32 = WIN_W - 2 * TAB_X;
/// A strip under the tabs for the version line.
const FOOTER_H: i32 = 18;
const TAB_H: i32 = WIN_H - 2 * TAB_Y - FOOTER_H;
/// Height of the tab strip itself; children start below it.
const TAB_BAR: i32 = 25;
const CONTENT_X: i32 = TAB_X + PAD;
const CONTENT_Y: i32 = TAB_Y + TAB_BAR + PAD;
const CONTENT_W: i32 = TAB_W - 2 * PAD;

/// What the start worker sends back to the UI thread.
#[derive(Clone)]
enum Msg {
    Started,
    StartFailed(String),
}

/// Mutable state the callbacks share. Every callback runs on the UI thread,
/// so a `RefCell` is the whole synchronisation story.
struct State {
    settings: Settings,
    runner: Runner,
    /// A start is in flight; the buttons stay disabled until it resolves.
    starting: bool,
}

pub fn run(log_buffer: LogBuffer) -> Result<(), String> {
    let runner = Runner::new().map_err(|e| format!("cannot create the HTTP runtime: {e}"))?;
    let state = Rc::new(RefCell::new(State {
        settings: Settings::load(),
        runner,
        starting: false,
    }));

    theme::apply();
    let _app = app::App::default();
    let (tx, rx) = app::channel::<Msg>();

    let mut win = Window::default()
        .with_size(WIN_W, WIN_H)
        .with_label("Faint Light")
        .center_screen();
    win.set_icon(window_icon());
    win.make_resizable(false);

    let mut tabs = Tabs::new(TAB_X, TAB_Y, TAB_W, TAB_H, None);
    tabs.set_selection_color(theme::FACE);
    let server = build_server_tab(&state, &tx);
    let logs_ui = build_logs_tab(&log_buffer);
    tabs.end();

    theme::note(
        CONTENT_X,
        TAB_Y + TAB_H + 2,
        CONTENT_W,
        FOOTER_H,
        &format!(
            "Version {} - nightsky.observer/faint-light",
            crate::version()
        ),
    );

    win.end();

    // Closing the last window ends the process inside FLTK, before the event
    // loop below gets another turn — so the shutdown work has to happen here,
    // in the callback, rather than after the loop.
    let quit = Rc::new(Cell::new(false));
    {
        let (state, server, quit) = (state.clone(), server.clone(), quit.clone());
        win.set_callback(move |w| {
            shutdown(&state, &server);
            quit.set(true);
            w.hide();
        });
    }
    win.show();

    load_settings_into(&state.borrow().settings, &server);
    refresh(&state.borrow(), &server);

    if state.borrow().settings.autostart {
        start_server(&state, &server, &tx);
    }

    let mut tray = tray::Tray::new();
    let mut log_view = LogView::new(logs_ui, log_buffer);
    // 0.15 s is a compromise: fast enough that the log tab feels live,
    // slow enough that an idle window costs nothing.
    while !quit.get() && app::wait_for(0.15).unwrap_or(false) {
        if let Some(msg) = rx.recv() {
            match msg {
                Msg::Started => {
                    state.borrow_mut().starting = false;
                    refresh(&state.borrow(), &server);
                }
                Msg::StartFailed(e) => {
                    state.borrow_mut().starting = false;
                    tracing::error!("server did not start: {e}");
                    refresh(&state.borrow(), &server);
                    server
                        .status
                        .clone()
                        .set_value(&format!("Not running - {e}"));
                    dialog::alert_default(&format!("The server did not start.\n\n{e}"));
                }
            }
        }
        log_view.pump();
        if tray.tick(&mut win) {
            quit.set(true);
        }
    }
    // Reached when the tray menu asked to quit; the window callback covers
    // the other way out.
    tray.shutdown(&mut win);
    shutdown(&state, &server);
    Ok(())
}

/// Persist the form and stop serving.
///
/// The form *is* the settings, so this takes them as they stand rather than
/// as the last Start captured them: unticking a box and closing has to
/// stick even though nothing was started afterwards. Safe to call twice.
fn shutdown(state: &Rc<RefCell<State>>, server: &Rc<ServerUi>) {
    collect_settings(server, &mut state.borrow_mut().settings);
    let st = state.borrow();
    if let Err(e) = st.settings.save() {
        tracing::warn!("could not save settings: {e}");
    }
    st.runner.stop();
}

/// The window icon, padded to a square.
///
/// The artwork is a wide eye (700x339). A window manager scales an icon into
/// a square slot without preserving its aspect ratio, which squashes it, so
/// pad it with transparency here rather than ship a second asset.
fn window_icon() -> Option<RgbImage> {
    let png = PngImage::from_data(ICON_PNG).ok()?;
    let (w, h) = (png.data_w() as usize, png.data_h() as usize);
    let depth = png.depth();
    let channels = depth as usize;
    let src = png.to_rgb_data();
    if w == 0 || h == 0 || channels == 0 || src.len() < w * h * channels {
        return None;
    }
    let side = w.max(h);
    let (ox, oy) = ((side - w) / 2, (side - h) / 2);
    let row = w * channels;
    let mut out = vec![0u8; side * side * channels];
    for y in 0..h {
        let from = y * row;
        let to = ((y + oy) * side + ox) * channels;
        out[to..to + row].copy_from_slice(&src[from..from + row]);
    }
    RgbImage::new(&out, side as i32, side as i32, depth).ok()
}

// ---------------------------------------------------------------------------
// Server tab

struct ServerUi {
    dot: Frame,
    status: Output,
    url: Frame,
    bind: Input,
    port: Input,
    index_dir: Input,
    cache: Input,
    timeout: Input,
    autostart: CheckButton,
    at_login: CheckButton,
    start: Button,
    stop: Button,
}

fn build_server_tab(state: &Rc<RefCell<State>>, tx: &app::Sender<Msg>) -> Rc<ServerUi> {
    let group = Group::new(TAB_X, TAB_Y + TAB_BAR, TAB_W, TAB_H - TAB_BAR, "Server\t");
    let mut y = CONTENT_Y;
    let half = LABEL_W + 90;

    theme::heading(CONTENT_X, y, CONTENT_W, "Server");
    theme::rule(CONTENT_X, y + ROW, CONTENT_W);
    y += ROW + GAP + 6;

    theme::label(CONTENT_X, y, LABEL_W, "Status:");
    let mut dot = Frame::new(CONTENT_X + LABEL_W, y + (ROW - 10) / 2, 10, 10, None);
    dot.set_frame(FrameType::FlatBox);
    dot.set_color(theme::STOPPED);
    let status = theme::output(CONTENT_X + LABEL_W + 16, y, CONTENT_W - LABEL_W - 16);
    y += ROW + GAP;

    theme::label(CONTENT_X, y, LABEL_W, "Bind address:");
    let bind = theme::input(CONTENT_X + LABEL_W, y, 90);
    theme::label(CONTENT_X + half + GAP, y, 40, "Port:");
    let port = theme::input(CONTENT_X + half + GAP + 44, y, 70);
    y += ROW + GAP;

    theme::label(CONTENT_X, y, LABEL_W, "Index directory:");
    let index_dir = theme::input(CONTENT_X + LABEL_W, y, CONTENT_W - LABEL_W - BUTTON_W - GAP);
    let mut browse = theme::button(
        CONTENT_X + CONTENT_W - BUTTON_W,
        y - 1,
        BUTTON_W,
        "Browse...",
    );
    y += ROW + GAP;

    theme::label(CONTENT_X, y, LABEL_W, "Index cache (GB):");
    let cache = theme::input(CONTENT_X + LABEL_W, y, 90);
    theme::label(CONTENT_X + half + GAP, y, 80, "Timeout (s):");
    let timeout = theme::input(CONTENT_X + half + GAP + 84, y, 70);
    y += ROW + GAP + 4;

    let mut autostart = CheckButton::new(
        CONTENT_X + LABEL_W,
        y,
        CONTENT_W - LABEL_W,
        ROW,
        "Start the server when this window opens",
    );
    autostart.set_frame(FrameType::NoBox);
    y += ROW;

    let at_login_label = if cfg!(windows) {
        "Start Faint Light when Windows starts"
    } else {
        "Start Faint Light at login"
    };
    let mut at_login = CheckButton::new(
        CONTENT_X + LABEL_W,
        y,
        CONTENT_W - LABEL_W,
        ROW,
        at_login_label,
    );
    at_login.set_frame(FrameType::NoBox);
    at_login.set_value(autostart::is_enabled());
    if !autostart::supported() {
        at_login.deactivate();
    }
    y += ROW + GAP + 6;

    let start = theme::button(CONTENT_X + LABEL_W, y, BUTTON_W, "Start");
    let stop = theme::button(CONTENT_X + LABEL_W + BUTTON_W + GAP, y, BUTTON_W, "Stop");
    y += ROW + GAP + 12;

    theme::rule(CONTENT_X, y, CONTENT_W);
    y += GAP + 2;
    let mut url = theme::note(CONTENT_X, y, CONTENT_W, ROW * 3, "");
    url.set_label_color(theme::NOTE);
    url.set_label_size(theme::FONT_SIZE + 2);

    group.end();

    let ui = Rc::new(ServerUi {
        dot,
        status,
        url,
        bind,
        port,
        index_dir,
        cache,
        timeout,
        autostart,
        at_login,
        start,
        stop,
    });

    {
        let (state, ui, tx) = (state.clone(), ui.clone(), *tx);
        let mut start_btn = ui.start.clone();
        start_btn.set_callback(move |_| start_server(&state, &ui, &tx));
    }
    {
        let (state, ui) = (state.clone(), ui.clone());
        let mut stop_btn = ui.stop.clone();
        stop_btn.set_callback(move |_| {
            state.borrow().runner.stop();
            refresh(&state.borrow(), &ui);
        });
    }
    {
        let mut index_dir = ui.index_dir.clone();
        browse.set_callback(move |_| {
            let mut chooser =
                dialog::NativeFileChooser::new(dialog::NativeFileChooserType::BrowseDir);
            chooser.set_title("Index files directory");
            let current = index_dir.value();
            if !current.is_empty() {
                let _ = chooser.set_directory(&PathBuf::from(current));
            }
            chooser.show();
            let path = chooser.filename();
            if !path.as_os_str().is_empty() {
                index_dir.set_value(&path.to_string_lossy());
            }
        });
    }
    {
        let mut at_login_btn = ui.at_login.clone();
        at_login_btn.set_callback(move |b| {
            if let Err(e) = autostart::set(b.value()) {
                tracing::warn!("could not change the login entry: {e}");
                dialog::alert_default(&format!("Could not change the startup entry.\n\n{e}"));
                // Show what is actually true, not what was clicked.
                b.set_value(autostart::is_enabled());
            }
        });
    }
    ui
}

/// Read the server fields, save them, and start on a worker thread —
/// loading index files can take tens of seconds.
fn start_server(state: &Rc<RefCell<State>>, ui: &Rc<ServerUi>, tx: &app::Sender<Msg>) {
    {
        let mut st = state.borrow_mut();
        if st.starting || st.runner.is_running() {
            return;
        }
        // Refuse to start on entries that cannot mean anything;
        // collect_settings keeps the stored value for the rest.
        if !matches!(ui.port.value().trim().parse::<u16>(), Ok(p) if p > 0) {
            dialog::alert_default("Port must be a number between 1 and 65535.");
            return;
        }
        if ui.bind.value().trim().parse::<std::net::IpAddr>().is_err() {
            dialog::alert_default(
                "Bind address must be an IP address, such as 0.0.0.0 (all \
                 interfaces) or 127.0.0.1 (this machine only).",
            );
            return;
        }
        collect_settings(ui, &mut st.settings);
        st.starting = true;
    }
    let st = state.borrow();
    let _ = st.settings.save();
    let cfg = st.settings.to_config();
    let runner = st.runner.clone();
    drop(st);

    ui.status
        .clone()
        .set_value("Starting - loading index files...");
    ui.start.clone().deactivate();
    ui.stop.clone().deactivate();

    let tx = *tx;
    std::thread::spawn(move || match runner.start(cfg) {
        Ok(_) => tx.send(Msg::Started),
        Err(e) => tx.send(Msg::StartFailed(e)),
    });
}

/// Copy the form into the settings. A field that is not currently a usable
/// entry keeps its stored value rather than being lost; `start_server`
/// validates the ones that must be right before it gets here.
fn collect_settings(ui: &ServerUi, s: &mut Settings) {
    if let Ok(port) = ui.port.value().trim().parse::<u16>() {
        if port > 0 {
            s.port = port;
        }
    }
    let bind = ui.bind.value().trim().to_string();
    if bind.parse::<std::net::IpAddr>().is_ok() {
        s.bind = bind;
    }
    s.index_dir = PathBuf::from(ui.index_dir.value().trim());
    if let Ok(gb) = ui.cache.value().trim().parse() {
        s.cache_gb = gb;
    }
    if let Ok(secs) = ui.timeout.value().trim().parse() {
        s.solve_timeout_secs = secs;
    }
    s.autostart = ui.autostart.value();
}

fn refresh(state: &State, ui: &ServerUi) {
    let running = state.runner.is_running();
    let mut dot = ui.dot.clone();
    dot.set_color(if running {
        theme::RUNNING
    } else {
        theme::STOPPED
    });
    dot.redraw();

    let mut status = ui.status.clone();
    let mut url = ui.url.clone();
    match state.runner.address() {
        Some(addr) => {
            status.set_value(&format!("Running - http://{addr}"));
            // 0.0.0.0 is not an address a client can dial, so show one that
            // is; the NINA field wants the /nova prefix specifically.
            let host = if addr.ip().is_unspecified() {
                format!("localhost:{}", addr.port())
            } else {
                addr.to_string()
            };
            url.set_label(&format!(
                "NINA and other astrometry.net clients:  http://{host}/nova\n\
                 Faint Light API:  http://{host}/api/v1/solve"
            ));
        }
        None => {
            status.set_value("Stopped");
            url.set_label("");
        }
    }
    url.redraw();

    let (mut start, mut stop) = (ui.start.clone(), ui.stop.clone());
    if state.starting {
        start.deactivate();
        stop.deactivate();
    } else if running {
        start.deactivate();
        stop.activate();
    } else {
        start.activate();
        stop.deactivate();
    }
}

fn load_settings_into(s: &Settings, ui: &ServerUi) {
    ui.bind.clone().set_value(&s.bind);
    ui.port.clone().set_value(&s.port.to_string());
    ui.index_dir
        .clone()
        .set_value(&s.index_dir.to_string_lossy());
    ui.cache.clone().set_value(&format!("{:.1}", s.cache_gb));
    ui.timeout
        .clone()
        .set_value(&s.solve_timeout_secs.to_string());
    ui.autostart.clone().set_value(s.autostart);
}

// ---------------------------------------------------------------------------
// Logs tab

struct LogsUi {
    display: TextDisplay,
    buffer: TextBuffer,
    follow: CheckButton,
}

fn build_logs_tab(log_buffer: &LogBuffer) -> LogsUi {
    let group = Group::new(TAB_X, TAB_Y + TAB_BAR, TAB_W, TAB_H - TAB_BAR, "Logs\t");
    let bottom = TAB_Y + TAB_H - PAD - ROW - 2;

    let buffer = TextBuffer::default();
    let mut display = TextDisplay::new(
        CONTENT_X,
        CONTENT_Y,
        CONTENT_W,
        bottom - CONTENT_Y - GAP,
        None,
    );
    display.set_buffer(buffer.clone());
    display.set_frame(FrameType::DownBox);
    display.set_color(Color::White);
    display.set_text_font(Font::Courier);
    display.set_text_size(theme::LOG_FONT_SIZE);
    // Log lines are longer than this window is wide, and a horizontal
    // scrollbar in a log viewer is a nuisance.
    display.wrap_mode(WrapMode::AtBounds, 0);
    // A TextDisplay has no editing at all, which is exactly the read-only
    // log view we want.

    let mut clear = theme::button(CONTENT_X, bottom, BUTTON_W, "Clear");
    let mut copy = theme::button(CONTENT_X + BUTTON_W + GAP, bottom, BUTTON_W, "Copy all");
    let mut follow = CheckButton::new(
        CONTENT_X + 2 * (BUTTON_W + GAP) + GAP,
        bottom,
        200,
        ROW + 2,
        "Follow new output",
    );
    follow.set_frame(FrameType::NoBox);
    follow.set_value(true);

    group.end();

    {
        let (log_buffer, mut buffer) = (log_buffer.clone(), buffer.clone());
        clear.set_callback(move |_| {
            log_buffer.clear();
            buffer.set_text("");
        });
    }
    {
        let buffer = buffer.clone();
        copy.set_callback(move |_| app::copy(&buffer.text()));
    }

    LogsUi {
        display,
        buffer,
        follow,
    }
}

/// Keeps the Logs tab in step with the shared buffer, splicing new lines
/// rather than rewriting the whole scrollback on every event.
struct LogView {
    ui: LogsUi,
    source: LogBuffer,
    version: u64,
    /// Lines appended since the last full rebuild, so the display cannot
    /// outgrow the buffer it mirrors.
    appended: usize,
}

impl LogView {
    fn new(ui: LogsUi, source: LogBuffer) -> LogView {
        let mut view = LogView {
            ui,
            source,
            version: 0,
            appended: 0,
        };
        view.rebuild();
        view
    }

    fn rebuild(&mut self) {
        let (version, text) = self.source.snapshot();
        self.ui.buffer.set_text(&text);
        self.version = version;
        self.appended = 0;
        self.follow();
    }

    fn pump(&mut self) {
        if self.source.version() == self.version {
            return;
        }
        match self.source.appended_since(self.version) {
            Some((version, text)) if self.appended < logs::CAPACITY => {
                self.appended += text.lines().count();
                self.ui.buffer.append(&text);
                self.version = version;
                self.follow();
            }
            _ => self.rebuild(),
        }
    }

    fn follow(&mut self) {
        if !self.ui.follow.value() {
            return;
        }
        let end = self.ui.buffer.length();
        self.ui.display.set_insert_position(end);
        self.ui
            .display
            .scroll(self.ui.buffer.count_lines(0, end), 0);
    }
}
