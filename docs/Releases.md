# Releases

Prebuilt binaries are attached to each
[GitHub release](https://github.com/fschuindt/faint_light/releases), one
archive per OS, architecture and flavour:

| | Linux x64 | Linux ARM64 | Windows x64 | Windows ARM64 |
|---|---|---|---|---|
| **headless** | ✓ | ✓ | ✓ | ✓ |
| **GUI** | ✓ | ✓ | ✓ | ✓ |

`headless` is the server on its own - what you want on a server, in a
container, or under a service manager. `gui` is the desktop window with the
server embedded, so it is the only file a desktop user needs. They are
separate executables for the reason described in [GUI.md](GUI.md).

Each archive carries the binary, `LICENSE`, `VERSION` and `README.md`.
Checksums for every archive are in `SHA256SUMS`.

## Platform support

The Windows builds target **Windows 10 and 11**. Windows 7 and 8 are not
supported: Rust's `*-pc-windows-msvc` targets have required Windows 10 since
Rust 1.78, and the tier-3 target that still reaches Windows 7 ships no
prebuilt standard library.

Linux builds are made on Ubuntu 22.04, so they need glibc 2.35 or newer.

## Versioning

The released version is the single line in [`VERSION`](../VERSION). It is what
the server banner, the GUI's footer and the `COMMENT` card of a solved FITS
all report. `Cargo.toml` keeps strict semver, which cannot express a suffix
like `0.1.0a`, so the two are deliberately separate.

## Cutting a release

1. Bump [`VERSION`](../VERSION) and commit.
2. Tag the commit `v<version>`, e.g. `git tag v0.1.0a && git push --tags`.
3. `.github/workflows/release.yml` builds all eight archives on native
   runners - cross-compiling FLTK's CMake/C++ tree is more trouble than a
   second runner - then runs each binary to check it starts, answers on both
   APIs and emits no warnings, and finally opens a **draft** release with the
   archives and `SHA256SUMS` attached.
4. Review the draft and publish it. Publishing is never automatic.

A rustc warning fails the build (`RUSTFLAGS: -D warnings`), so a release
cannot carry one.
