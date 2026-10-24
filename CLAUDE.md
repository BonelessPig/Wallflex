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
- `src/config.rs`: parses and validates `config.toml` (serde + toml)
  into `Settings`: background, and layouts matched to monitors by
  `*` / `primary` / resolution / `DISPLAYn` (first match wins), each
  placing widgets by `Anchor` + offset in design units. `Anchor::place`
  positions within the work area and clamps to the monitor.
  `config.default.toml` is the commented default, written on first run
  and checked by a test to equal `Settings::default()`.
- `src/log.rs`: `log!` writes to stderr and `wallflex.log` next to the
  config. Use it instead of `eprintln!` in the running app.
- `src/discord/`: safe Discord code, ported from the user's
  DiscordBridge. `rpc.rs` frames the local pipe protocol over a
  `Transport` that also delivers UI commands (one blocking wait, no
  polling). `session.rs` signs in (OAuth token refresh), follows your
  call, speaking, notifications and the SERVER page's channels.
  `bot.rs` is the gateway protocol and member/presence cache for the
  optional bot. `model.rs` is the view types. All unit-tested with
  fake transports.
- `src/widgets/discord.rs` (+ `discord/server.rs`, `discord/friends.rs`):
  the Discord widget redrawn from the wallpaper's `discord.js`: tabs,
  VOICE (same name-row rules), SERVER, FRIENDS, INBOX, summary line,
  offline icon. It records hit areas while drawing.
- `src/image.rs`: decoded pictures (premultiplied BGRA).
- `src/win32/`: the Win32 layer.
  - `desktop.rs`: finds where to draw. Explorer's split desktop has two
    shapes: **Raised** (Windows 11 24H2+: surfaces are layered children
    of `Progman`, directly below `SHELLDLL_DefView`) and **Classic**
    (surfaces inside the wallpaper `WorkerW`). Undocumented; message
    `0x052C` with wParam `0xD` asks Explorer to split.
  - `app.rs`: hidden controller window and message loop. Sensors and a
    config-file change check every 1 s; health check every 5 s; rebuilds
    on config change, Explorer restart (`TaskbarCreated`), display change
    and resume. A bad config is logged and the old settings kept.
  - `surface.rs`: one window per monitor. `paint.rs`: GDI background
    plus the layout's widgets, each with its own persistent DIB section:
    the canvas draws straight into it, GDI adds the text (Bahnschrift,
    built into Windows; the wallpaper's own font isn't ours to ship) and
    BitBlts it in. Only the widgets' areas are repainted each second.
    Scale = monitor height / 1000 (at least 1 px per unit) x layout size.
  - `monitors.rs`: each monitor's rectangle, work area, name and primary
    flag.
  - `discord/`: the session thread plus a pipe-reader thread
    (overlapped I/O on `\\.\pipe\discord-ipc-N`), the bot thread plus a
    websocket-reader thread (`gateway.rs`), and `focus.rs` (raise
    Discord's window after opening a `discord://` link).
  - `http.rs`: WinHTTP requests and websockets (no TLS library).
    `images.rs`: download + WIC decode on a thread, with retries.
  - `tray.rs`: the notification-area icon (drawn with the canvas at the
    small-icon size) and its menu; `app.rs` handles the choices and
    re-adds the icon when Explorer restarts. `startup.rs`: "Start with
    Windows" as an HKCU `Run` value.
  - Clicks: a WH_MOUSE_LL hook (only while Discord is set up) notes a
    press and release at the same spot, and only acts when the desktop
    itself is under the pointer. It never swallows clicks.
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
- `--preview <h> <file> --widget discord` (or `discord-inbox`) draws
  the Discord widget with sample data and real downloaded avatars.
- Discord credentials and the bot token live in `[discord]` in the
  config; the OAuth token is `discord-token.json` next to it. Never
  print or commit them.
- `tools\package.ps1` builds `dist\wallflex-<version>.zip` (exe,
  `docs/SETUP.md`, the default config, licences). `docs/SETUP.md` is
  the friend-facing guide: keep it in step with the config and menu.
- Test layouts with `--config <file>` instead of editing the user's
  real `%APPDATA%\Wallflex\config.toml`; the log goes next to that
  file.
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

Milestone 4 (23 Oct 2026, release build, 60 s, default config): 16.0 MB
working set, 8.8 MB private, 0.08% CPU, exe 417 KB (was 240 KB; serde +
toml). Anchors, live reload and error reporting were tested on the
upper monitor with a test config; plugging monitors in and out hasn't
been tested on real hardware yet (it goes through the same
WM_DISPLAYCHANGE rebuild).

Milestone 5 (23 Oct 2026, release build, 60 s, Discord + bot in 9
servers following 34 friends): 17.6 MB working set, 8.9 MB private,
0.1% CPU, exe 760 KB. Tested live: sign-in with a stored token, tabs and
clicks, the server list and a server's channels, FRIENDS with real
presences and avatars. Not yet tested live: being in a call (speaking
rings, camera badges) and opening a notification, since that needs a
call or a message to arrive.

Milestone 6 (23 Oct 2026, release build): the tray icon adds about
1.2 MB working set and 0.4-0.8 MB private (shell32's notification-area
DLLs). Default config: 18.7 MB working set, 9.6 MB private, 0.07% CPU.
With Discord + bot while in a call: 28.1 MB working set, 12.8 MB
private, 0.12% CPU; the milestone 5 build measured 27 MB / 12 MB under
the same conditions, so the call, not the tray, is most of that (to
look into). Exe 768 KB, release zip 397 KB. Tested live: tray menu,
Start with Windows on and off (Run value written and removed), Quit.
