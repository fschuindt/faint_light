# Desktop GUI

An optional FLTK window, off by default. It configures and runs the embedded
server and shows its log; solving belongs to the API and to the
web UI the server serves at its root, which the window's
**Open web UI** button opens in your browser.

Two tabs:

- **Server** - status, bind address, port, index directory, cache budget and
  solve timeout, with Start, Stop and Open web UI. Changing the port or the
  address rebinds the listener in place; the loaded index files stay in
  memory, so a restart is instant.
- **Logs** - the same `tracing` output the headless binary prints, read-only.

Minimising the window puts it in the notification area (Windows only so far)
and leaves the server running. Click the icon, or its **Open Faint Light**
item, to bring the window back; **Exit** quits.

## Building it

```bash
cargo build --release -p fl-server --features gui
```

FLTK is compiled from source, which needs **CMake 3.28 or newer** and a C++
toolchain: MSVC on Windows, gcc or clang plus the X11 (or Wayland) development
headers on Linux. `--features gui-bundled` links prebuilt FLTK binaries
instead, for the targets fltk-rs publishes them for.

## Where it keeps things

| What | Where |
|---|---|
| Settings | `%APPDATA%\faint_light\gui.json`, or `~/.config/faint_light/gui.json` |
| Log | `faint-light.log`, in the same directory |
| Start at login | `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`, or `~/.config/autostart/faint-light.desktop` |

Environment variables supply the defaults for the settings file, and the
window writes it when you press Start or close the window. Anything the window
does not expose - the pixel-scale prior, `FAINT_LIGHT_FAKE` - keeps coming
from the environment.

The Logs tab is backed by the log file, so it still shows what happened in
previous runs. **Clear** empties the tab and the file together; nothing else
discards history.

"Start Faint Light when Windows starts" is not kept in the settings file. The
platform's own autostart entry is the source of truth, so the checkbox always
agrees with what the system will actually do.

The headless server reads no file at all: environment variables are its only
configuration.

## Why two executables

`faint-light` and `faint-light-gui` are deliberately separate. A Windows GUI
has to be linked into the `windows` subsystem or it opens a console window
behind itself, while the headless server has to stay a console program so
Ctrl-C, redirected output and service wrappers work. That is a link-time
choice, so one executable cannot be both.

Keeping them apart also keeps FLTK, CMake and a C++ toolchain out of the
server build, which is what lets the container image stay small.
