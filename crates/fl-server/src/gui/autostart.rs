//! "Start Faint Light when the computer starts".
//!
//! The platform's own autostart mechanism is the single source of truth —
//! an HKCU `Run` value on Windows, an XDG `.desktop` file elsewhere — so the
//! checkbox never disagrees with what will actually happen, even if someone
//! removes the entry behind the program's back.

/// Whether this build can register itself to run at login.
pub fn supported() -> bool {
    cfg!(any(windows, target_os = "linux")) && exe_path().is_some()
}

fn exe_path() -> Option<std::path::PathBuf> {
    std::env::current_exe().ok()
}

#[cfg(windows)]
mod imp {
    use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_WRITE};
    use winreg::RegKey;

    const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    const VALUE: &str = "FaintLight";

    fn open(write: bool) -> Result<RegKey, String> {
        let access = if write {
            KEY_READ | KEY_WRITE
        } else {
            KEY_READ
        };
        RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey_with_flags(RUN_KEY, access)
            .map_err(|e| format!(r"HKCU\{RUN_KEY}: {e}"))
    }

    pub fn is_enabled() -> bool {
        open(false)
            .and_then(|k| k.get_value::<String, _>(VALUE).map_err(|e| e.to_string()))
            .is_ok()
    }

    pub fn set(on: bool) -> Result<(), String> {
        let key = open(true)?;
        if on {
            let exe = super::exe_path().ok_or("cannot locate this executable")?;
            // Quoted: the path routinely contains spaces.
            key.set_value(VALUE, &format!("\"{}\"", exe.display()))
                .map_err(|e| e.to_string())
        } else {
            match key.delete_value(VALUE) {
                Ok(()) => Ok(()),
                // Already absent is the state the caller asked for.
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(e.to_string()),
            }
        }
    }
}

#[cfg(all(unix, target_os = "linux"))]
mod imp {
    use std::path::PathBuf;

    fn desktop_file() -> PathBuf {
        super::super::settings::config_dir()
            .join("autostart")
            .join("faint-light.desktop")
    }

    pub fn is_enabled() -> bool {
        desktop_file().is_file()
    }

    pub fn set(on: bool) -> Result<(), String> {
        let path = desktop_file();
        if !on {
            return match std::fs::remove_file(&path) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(format!("{}: {e}", path.display())),
            };
        }
        let exe = super::exe_path().ok_or("cannot locate this executable")?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        let entry = format!(
            "[Desktop Entry]\nType=Application\nName=Faint Light\n\
             Comment=Plate-solving server\nExec=\"{}\"\nTerminal=false\n\
             X-GNOME-Autostart-enabled=true\n",
            exe.display()
        );
        std::fs::write(&path, entry).map_err(|e| format!("{}: {e}", path.display()))
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
mod imp {
    pub fn is_enabled() -> bool {
        false
    }
    pub fn set(_on: bool) -> Result<(), String> {
        Err("not supported on this platform".into())
    }
}

pub use imp::{is_enabled, set};
