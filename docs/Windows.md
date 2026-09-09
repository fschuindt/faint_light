# Windows

The server, the debugging commands and the tests build and run natively on
Windows with the MSVC toolchain — no WSL, no MSYS.

```powershell
$env:FAINT_LIGHT_INDEX_DIR = "C:\astrometry\indexes"
cargo run --release -p fl-server
```

Prebuilt binaries for Windows x64 and ARM64 are attached to each release; see
[Releases.md](Releases.md), which also states the supported Windows versions.

## Firewall

The listener binds `0.0.0.0` by default, so Windows Firewall asks once whether
to allow it. Allow it if a capture machine elsewhere on the LAN will use the
server; declining still leaves it reachable from the same PC. Set
`FAINT_LIGHT_BIND=127.0.0.1` to skip the question entirely.

## Testing a submission

`scripts/test_submission.sh` needs a shell and `jq`. The PowerShell twin needs
neither — it uses `curl.exe`, which ships with Windows 10 and later:

```powershell
.\scripts\test_submission.ps1 path\to\image.jpg localhost:7222
```

## Building the GUI

`--features gui` compiles FLTK from source, which needs CMake 3.28 or newer
and the MSVC C++ toolchain. The CMake bundled with Visual Studio 2019 is 3.20
and will be rejected; install a current one (`winget install Kitware.CMake`)
or use `--features gui-bundled`, which links prebuilt FLTK binaries instead.

See [GUI.md](GUI.md) for what the window does and where it stores its files.
