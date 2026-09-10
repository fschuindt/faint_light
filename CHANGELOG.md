# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html),
with a trailing letter marking the alpha/beta series (see [docs/Releases.md](docs/Releases.md)).

## [Unreleased]

### Added
- Every release archive is now also attached under a name without the
  version in it (`faint-light-windows-x64-gui.zip` and the other seven), so
  a fixed link of the form `releases/latest/download/<name>` always fetches
  the current release. The copies are byte-identical and listed in
  `SHA256SUMS`.

## [0.1.1a] - 2026-09-10

### Fixed
- The Linux desktop GUI exited on its own, taking the embedded server with
  it, as soon as the window had been idle for 150 ms. FLTK's `Fl::wait(time)`
  returns 1 on Windows but zero elsewhere when nothing happens, and the event
  loop had treated that return value as "keep going". The window being closed
  is now what ends the loop.
- The release verification script prints the GUI's own log file and the
  process state when the server never answers, so a failure like the one
  above is visible in the job log instead of an empty console section.

## [0.1.0a] - 2026-09-09

First public release.

### Added
- A blind plate solver written in Rust that memory-maps stock astrometry.net
  index files (4100 and 4200 series, the healpix-tiled Gaia 5000/6000 series)
  and walks their kd-trees directly, with no C dependencies and no format
  conversion.
- Warm start: the last few solves are remembered, so a nearby next frame is
  verified against the previous solution instead of searched again, and a
  frame elsewhere still benefits from the known pixel scale. An in-memory
  index cache with a configurable RAM budget (`FAINT_LIGHT_CACHE_GB`).
- A `nova.astrometry.net`-compatible HTTP API under `/nova`, so NINA and
  other astrometry.net clients work by pointing them at the server.
- Faint Light's own `/api/v1/solve`, which can also return the image as a
  FITS carrying the solved WCS, a preview PNG, and an all-sky chart (SVG) of
  where the frame was taken. Described in `openapi.yml`.
- A web UI served at the root, to plate-solve from a browser.
- An optional desktop GUI (`faint-light-gui`, `--features gui`): a window to
  configure, start and stop the server and read its log, with
  minimise-to-tray and start-at-login on Windows.
- Native Windows builds (x64 and ARM64) alongside Linux x64 and ARM64, in
  headless and GUI flavours, built and verified by GitHub Actions on every
  tag; a Docker image and `docker-compose.yml` for the headless server.
- Documentation: README, `docs/GUI.md`, `docs/Windows.md`, `docs/Releases.md`,
  and a public roadmap.

[Unreleased]: https://github.com/fschuindt/faint_light/compare/v0.1.1a...HEAD
[0.1.1a]: https://github.com/fschuindt/faint_light/compare/v0.1.0a...v0.1.1a
[0.1.0a]: https://github.com/fschuindt/faint_light/releases/tag/v0.1.0a
