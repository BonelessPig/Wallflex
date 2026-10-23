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
- `clippy.toml` lets tests unwrap, expect, panic and index: a panic is
  how a test fails. Plain arithmetic is still linted in tests.
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
- `src/sensors.rs`: the `Snapshot` types and the pure sensor maths
  (rates, GPU usage by Task Manager's method, storage descriptor
  parsing). `src/text.rs`: snapshot to text lines for `--sensors`.
- `src/widgets.rs`: the widgets, redrawn from the user's wallpaper
  designs (`cpu-cores.js`, `net-graph.js`, `gpu-rings.js` + CSS in
  `PerformanceMonitor\wallpaper`). Sizes are design units: 1 unit = 0.1%
  of the monitor height (the wallpaper's 0.1vh), also the line weight.
  `Dashboard` holds the latest reading and a minute of network history,
  draws shapes to a `Canvas` and returns text as `Label`s.
- `src/canvas.rs`: a small anti-aliased software rasteriser (rects,
  ring arcs, polylines, area fill) over a borrowed pixel slice. No
  graphics library: Direct2D and GDI+ cost far more memory.
- `src/bmp.rs`: BMP writer for `--preview`.
- `src/win32/`: the Win32 layer.
  - `desktop.rs`: finds where to draw. Explorer's split desktop has two
    shapes: **Raised** (Windows 11 24H2+: surfaces are layered children
    of `Progman`, directly below `SHELLDLL_DefView`) and **Classic**
    (surfaces inside the wallpaper `WorkerW`). Undocumented; message
    `0x052C` with wParam `0xD` asks Explorer to split.
  - `app.rs`: hidden controller window and message loop. Sensors every
    1 s (repaints only the panel); health check every 5 s; rebuilds on
    Explorer restart (`TaskbarCreated`), display change and resume.
  - `surface.rs`: one window per monitor. `paint.rs`: GDI background
    plus the widgets: the canvas draws straight into a persistent DIB
    section sized to the widgets, GDI adds the text (Bahnschrift, built
    into Windows; the wallpaper's own font isn't ours to ship) and
    BitBlts it in. Only the widgets' area is repainted each second.
  - `sensors/`: raw readings, all from built-in Windows sources with no
    admin: `pdh.rs` (CPU per core and clock, GPU engine use, VRAM in
    use), `gpu.rs` (D3DKMT: temperature, fan, power, VRAM size),
    `net.rs` (interface byte counters), `disk.rs` (SSD temperature
    query; hard drives skipped so they never spin up). Ported from the
    user's own `windows_sensors.py`.
- `wallflex.manifest`: per-monitor DPI v2 and Windows 8+ support (needed
  for layered child windows); embedded by `build.rs`.

## Running and checking

- `wallflex --preview <height> <file.bmp>` renders the widgets with live
  data as they'd look on a monitor that tall: the way to check a
  change at 1080, 1440 and 2160 without anything on screen.
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
windows (or buffers) sized to the widget. The Direct2D modes were removed
in milestone 2.

Milestone 2 (23 Oct 2026, release build, 60 s, sensors every second):
13.5 MB working set, 6.4 MB private, 0.07% of the CPU (0.86% of one
core), 3 threads, no GPU driver. The PDH libraries account for most of
the growth from milestone 1. Readings match the old PerformanceMonitor
service (GPU, SSD, RAM, network exactly; CPU within a few points, since
the two can't sample the same instant).

Milestone 3 (23 Oct 2026, release build, 60 s, widgets on both
monitors): 15.8 MB working set (just over the 15 MB target), 8.5 MB
private, 0.07% CPU (0.81% of one core), 3 threads, no GPU driver. The
widget buffers are ~3 MB (2 MB at 4K); most of the rest is the PDH
libraries. The first version cost 0.25% CPU until polylines only
visited pixels near each segment and arcs only their band.
