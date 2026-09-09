//! Minimise to the notification area.
//!
//! Windows only for now. The Linux tray protocol (StatusNotifierItem over
//! D-Bus) would pull GTK into an FLTK program, which is a heavier dependency
//! than the feature is worth; elsewhere minimising behaves normally and this
//! is a no-op.

use fltk::window::Window;

/// Watches the window and moves it to the notification area when the user
/// minimises it.
#[derive(Default)]
pub struct Tray(imp::Tray);

impl Tray {
    pub fn new() -> Tray {
        Tray(imp::Tray::new())
    }

    /// Call once per event-loop tick. Returns true when the user chose Exit
    /// from the tray menu.
    pub fn tick(&mut self, win: &mut Window) -> bool {
        self.0.tick(win)
    }

    /// Take the icon down and put the window back on screen. Call before
    /// exiting, so nothing is left in the notification area.
    pub fn shutdown(&mut self, win: &mut Window) {
        self.0.shutdown(win);
    }
}

#[cfg(windows)]
mod imp {
    use std::cell::RefCell;

    use fltk::prelude::WindowExt;
    use fltk::window::Window;

    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::Shell::{
        Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DestroyWindow,
        GetCursorPos, IsIconic, LoadIconW, RegisterClassW, SetForegroundWindow, ShowWindow,
        TrackPopupMenu, HWND_MESSAGE, IDI_APPLICATION, MF_STRING, SW_HIDE, SW_RESTORE, SW_SHOW,
        TPM_RETURNCMD, TPM_RIGHTBUTTON, WM_APP, WM_LBUTTONDBLCLK, WM_LBUTTONUP, WM_RBUTTONUP,
        WNDCLASSW,
    };

    /// Our callback message. The notification area sends it to `msg_hwnd`
    /// with the mouse event in `lparam`.
    const WM_TRAY: u32 = WM_APP + 1;
    const MENU_OPEN: usize = 1;
    const MENU_EXIT: usize = 2;
    /// The icon id winresource gives the executable's main icon.
    const APP_ICON_ID: u16 = 1;

    enum Event {
        Restore,
        Quit,
    }

    // The window procedure runs on this same thread, dispatched out of
    // FLTK's own message pump, so a thread-local queue is all the handoff
    // that is needed.
    thread_local! {
        static EVENTS: RefCell<Vec<Event>> = const { RefCell::new(Vec::new()) };
    }

    #[derive(Default)]
    pub struct Tray {
        /// Message-only window that receives the notification callbacks.
        /// Its own procedure keeps us out of FLTK's.
        msg_hwnd: HWND,
        /// The icon is in the tray and the window is hidden.
        in_tray: bool,
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    unsafe extern "system" fn wndproc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        if msg == WM_TRAY {
            match lparam as u32 {
                WM_LBUTTONUP | WM_LBUTTONDBLCLK => push(Event::Restore),
                WM_RBUTTONUP => unsafe { context_menu(hwnd) },
                _ => {}
            }
            return 0;
        }
        unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
    }

    fn push(e: Event) {
        EVENTS.with(|q| {
            if let Ok(mut q) = q.try_borrow_mut() {
                q.push(e);
            }
        });
    }

    /// The right-click menu. `TPM_RETURNCMD` hands the choice straight back,
    /// so there is no WM_COMMAND to route.
    unsafe fn context_menu(hwnd: HWND) {
        unsafe {
            let menu = CreatePopupMenu();
            if menu.is_null() {
                return;
            }
            AppendMenuW(
                menu,
                MF_STRING,
                MENU_OPEN,
                wide("&Open Faint Light").as_ptr(),
            );
            AppendMenuW(menu, MF_STRING, MENU_EXIT, wide("E&xit").as_ptr());
            let mut pt = POINT { x: 0, y: 0 };
            GetCursorPos(&mut pt);
            // Required so the menu closes when the user clicks elsewhere.
            SetForegroundWindow(hwnd);
            let choice = TrackPopupMenu(
                menu,
                TPM_RETURNCMD | TPM_RIGHTBUTTON,
                pt.x,
                pt.y,
                0,
                hwnd,
                std::ptr::null(),
            );
            DestroyMenu(menu);
            match choice as usize {
                MENU_OPEN => push(Event::Restore),
                MENU_EXIT => push(Event::Quit),
                _ => {}
            }
        }
    }

    impl Tray {
        pub fn new() -> Tray {
            let class = wide("FaintLightTray");
            unsafe {
                let instance = GetModuleHandleW(std::ptr::null());
                let mut wc: WNDCLASSW = std::mem::zeroed();
                wc.lpfnWndProc = Some(wndproc);
                wc.hInstance = instance;
                wc.lpszClassName = class.as_ptr();
                // A duplicate registration is fine; the class is per-process
                // and we only ever make one window from it.
                RegisterClassW(&wc);
                let msg_hwnd = CreateWindowExW(
                    0,
                    class.as_ptr(),
                    std::ptr::null(),
                    0,
                    0,
                    0,
                    0,
                    0,
                    HWND_MESSAGE,
                    std::ptr::null_mut(),
                    instance,
                    std::ptr::null(),
                );
                Tray {
                    msg_hwnd,
                    in_tray: false,
                }
            }
        }

        fn icon_data(&self) -> NOTIFYICONDATAW {
            unsafe {
                let mut nid: NOTIFYICONDATAW = std::mem::zeroed();
                nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
                nid.hWnd = self.msg_hwnd;
                nid.uID = 1;
                nid.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
                nid.uCallbackMessage = WM_TRAY;
                let instance = GetModuleHandleW(std::ptr::null());
                // The executable's own icon, or the stock one if the
                // resource is missing (no resource compiler at build time).
                nid.hIcon = LoadIconW(instance, APP_ICON_ID as *const u16);
                if nid.hIcon.is_null() {
                    tracing::warn!("tray: exe icon resource missing, using the stock one");
                    nid.hIcon = LoadIconW(std::ptr::null_mut(), IDI_APPLICATION);
                }
                for (slot, ch) in nid.szTip.iter_mut().zip(wide("Faint Light")) {
                    *slot = ch;
                }
                nid
            }
        }

        fn window(win: &mut Window) -> HWND {
            win.raw_handle() as HWND
        }

        fn hide_to_tray(&mut self, win: &mut Window) {
            if self.msg_hwnd.is_null() {
                return; // no message window: leave the minimise alone
            }
            unsafe {
                if Shell_NotifyIconW(NIM_ADD, &self.icon_data()) == 0 {
                    // Leave the window minimised the ordinary way rather
                    // than hiding it with nothing to bring it back.
                    tracing::warn!("tray: the notification area refused the icon");
                    return;
                }
                // Hiding behind FLTK's back is deliberate: FLTK still counts
                // the window as shown, so the event loop keeps running while
                // the program lives only in the notification area.
                ShowWindow(Self::window(win), SW_HIDE);
            }
            self.in_tray = true;
            tracing::debug!("minimised to the notification area");
        }

        fn show_from_tray(&mut self, win: &mut Window) {
            if !self.in_tray {
                return;
            }
            unsafe {
                Shell_NotifyIconW(NIM_DELETE, &self.icon_data());
                let hwnd = Self::window(win);
                ShowWindow(hwnd, SW_SHOW);
                ShowWindow(hwnd, SW_RESTORE);
                SetForegroundWindow(hwnd);
            }
            self.in_tray = false;
        }

        pub fn tick(&mut self, win: &mut Window) -> bool {
            if !self.in_tray && unsafe { IsIconic(Self::window(win)) } != 0 {
                self.hide_to_tray(win);
            }
            let events: Vec<Event> = EVENTS.with(|q| q.borrow_mut().drain(..).collect());
            let mut quit = false;
            for e in events {
                match e {
                    Event::Restore => self.show_from_tray(win),
                    Event::Quit => quit = true,
                }
            }
            quit
        }

        pub fn shutdown(&mut self, win: &mut Window) {
            self.show_from_tray(win);
            if !self.msg_hwnd.is_null() {
                unsafe { DestroyWindow(self.msg_hwnd) };
                self.msg_hwnd = std::ptr::null_mut();
            }
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use fltk::window::Window;

    #[derive(Default)]
    pub struct Tray;

    impl Tray {
        pub fn new() -> Tray {
            Tray
        }
        pub fn tick(&mut self, _win: &mut Window) -> bool {
            false
        }
        pub fn shutdown(&mut self, _win: &mut Window) {}
    }
}
