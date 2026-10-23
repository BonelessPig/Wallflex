# CLAUDE.md

Guidance for Claude Code when working in this repository.

## What this is

Wallflex: lightweight desktop widgets for Windows, drawn by one native
Rust process between the wallpaper and the desktop icons. It replaces
DiscordBridge, the PerformanceMonitor fork, the private ui-particles
wallpaper and Wallpaper Engine. Plan:
https://claude.ai/artifact/5PTjEbTctywWvkTJDFbAuy

**Lightweight comes first.** Budget: under 15 MB RAM, under 0.5% CPU, no
GPU use between repaints. Measure changes with `tools\measure.ps1`.

License: MIT OR Apache-2.0. Only the user's own code may come in from the
old repos (DiscordBridge, `windows_sensors.py`, their widget JS); the
fork's core and the Workshop authors' files are not licensed to copy.

## Standards (all enforced; a warning fails the build)

- `cargo fmt` (max width 80, from `rustfmt.toml`).
- `cargo clippy --all-targets -- -D warnings`, with `pedantic` plus the
  MISRA-style lints in `Cargo.toml` (no unwrap/expect/panic, no unchecked
  indexing, overflow or `as` casts).
- `unsafe` is denied everywhere except `src/win32/`, which opts in. Every
  `unsafe` block needs a `// SAFETY:` comment saying why it is sound.
- Every item, private ones included, has a doc comment.
- Check every fallible result. Ignore one only with `let _ =` and a
  comment saying why that's fine.
- No magic numbers: named constants.
- Lines: aim for 80 characters, never over 100.

Run before calling work done:

```
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

Rust lives in `%USERPROFILE%\.cargo\bin`; in Git Bash prepend it to
`PATH` if `cargo` isn't found.

## Structure

- `src/main.rs`: entry point, dispatches on the command.
- `src/args.rs`, `src/color.rs`, `src/geometry.rs`: safe, unit-tested
  logic.
- `src/win32/`: the Win32 layer.
  - `desktop.rs`: finds where to draw. Explorer's split desktop has two
    shapes: **Raised** (Windows 11 24H2+: surfaces are layered children
    of `Progman`, directly below `SHELLDLL_DefView`) and **Classic**
    (surfaces inside the wallpaper `WorkerW`). Undocumented; message
    `0x052C` with wParam `0xD` asks Explorer to split.
  - `app.rs`: hidden controller window and message loop. Health check
    every 5 s; rebuilds on Explorer restart (`TaskbarCreated`), display
    change and resume from sleep.
  - `surface.rs`: one window per monitor. `render.rs`: GDI or Direct2D.
- `wallflex.manifest`: per-monitor DPI v2 and Windows 8+ support (needed
  for layered child windows); embedded by `build.rs`.

## Running and checking

- `cargo run -- --color FF00FF` makes the surfaces obvious.
  `target\debug\wallflex.exe --diagnose` lists `Progman`'s children
  front to back; the surfaces must sit between `SHELLDLL_DefView` and
  `WorkerW`.
- Stop a running copy with `wallflex --quit`. Only one copy runs at a
  time.
- Don't restart Explorer or put the PC to sleep to test without asking
  the user first: restarting Explorer closes their File Explorer windows.

## Findings so far

Milestone 1 (23 Oct 2026, release build, both monitors):

| Render mode | Working set | Private | CPU | Threads |
| --- | --- | --- | --- | --- |
| GDI | 6.9 MB | 1.1 MB | 0% | 1 |
| Direct2D, CPU | 115.5 MB | 106 MB | 0% | 15 |
| Direct2D, GPU | 34.7 MB | 107 MB | 0% | 34 |

Direct2D keeps a buffer the size of each monitor, so it must not be used
for full-monitor windows. Paint backgrounds with GDI; draw widgets in
windows (or buffers) sized to the widget.
