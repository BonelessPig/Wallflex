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
- CPU columns follow the real core layout (`GetLogicalProcessorInformationEx`,
  read once), and per-thread loads are kept by processor number, so a
  PDH reading that skips an instance can't shift or regroup the
  columns (it used to show 5 cores or 11 single threads).
- `src/sensors.rs`: the `Snapshot` types and the pure sensor maths
  (rates, GPU usage by Task Manager's method, storage descriptor
  parsing). `src/text.rs`: snapshot to text lines for `--sensors`.
- `src/widgets.rs`: the widgets, redrawn from the user's wallpaper
  designs (`cpu-cores.js`, `net-graph.js`, `gpu-rings.js` + CSS in
  `PerformanceMonitor\wallpaper`). Sizes are design units: 1 unit = 0.1%
  of the monitor height (the wallpaper's 0.1vh), also the line weight.
  `Dashboard` holds the latest reading and the network history
  (`[network] seconds`, 30 by default; drawn as rounded, filled
  curves), draws shapes to a `Canvas` and returns text as `Label`s.
- `src/pace.rs`: performance. The config's `performance` ("smooth"
  default, or "light") caps a `Level` (Smooth: 0.4 s glides and
  visualizer at ~64 fps; Normal: 0.3 s / ~32 fps; Light: no glides,
  sensors once a second, visualizer ~16 fps). The `Governor` drops
  straight to Light at once when the PC is 80% busy, steps down a level
  when Wallflex is over 2% CPU for 3 s (`win32/usage.rs` measures it
  by cycle count, not the tick-sampled process times),
  and climbs back one level per 10 calm seconds. Timer intervals sit
  just under whole 15.625 ms system ticks (15, 31, 62 ms), since a
  timer fires on the first tick after it's due. Over 120 MB private,
  pictures are dropped and the working set trimmed (health tick).
- `src/widgets/motion.rs`: the glide. After each reading the CPU
  columns, GPU rings and RAM bar ease to the new values over 0.3 s
  (text changes at once); `Dashboard::moving_areas` says which parts
  move. Previews call `settle()`.
- `src/canvas.rs`: a small anti-aliased software rasteriser (rects,
  ring arcs, polylines, area fill) over a borrowed pixel slice, with an
  optional clip rectangle. No
  graphics library: Direct2D and GDI+ cost far more memory.
- `src/bmp.rs`: BMP writer for `--preview`.
- `src/bin/wallflex-settings/`: the optional settings app, a second
  binary using only std + serde_json + toml (no `unsafe`, no Win32).
  `server.rs` serves `ui.html` on `127.0.0.1:<random port>`; API calls
  need the token from the page's address (`X-Wallflex-Token`) and a
  matching `Host`. It never validates or renders itself: it runs
  `wallflex.exe --check`, `--info` and `--preview 1000 <dir> --widget
  all --config <draft>` next to it, so it can't disagree with the
  app. Saving checks first, refuses if the file's stamp changed since
  loading (409), keeps `config.toml.bak` and swaps the file in by
  rename. The page heartbeats; the app exits ~4 s after the window
  closes (5 min of silence otherwise). `ui.html` mirrors
  `Anchor::place` and monitor matching in JS; drags re-pick the anchor
  by which third the widget's centre is in. It writes TOML itself
  (the user's comments are not kept). `POST /api/picture` (body: the
  path) returns a picture file so the page can draw it on the
  monitors; only picture extensions, up to 128 MB. "Real size" draws
  each monitor shrunk by its pitch about its shared edges (from
  `--info`'s `mm`), so a span lines up there as on the desk. `window.rs` opens Edge with
  `--app=`, else the default browser. The tray's "Open settings"
  starts it with `--config` when it sits next to `wallflex.exe`, else
  opens the file. Test with `--no-open --config <scratch file>` and
  curl (copy the token from its output).
- `src/backdrop.rs`: what's behind the widgets: `Background` (a colour
  or `Transparent`), an optional picture (`Wallpaper`: path, `Fit`
  (fill, fit, stretch, center, span), zoom, offset), and the placing:
  `views` gives each monitor a `View` (the picture fitted to a canvas:
  the monitor, or for span the box around every monitor sharing the
  picture; then zoomed about an anchor and shifted), and `map` turns a
  view into a `Mapping` (scaled size, source and dest rectangles).
  Span anchors are the middle of the edges a monitor shares with the
  others, so zoom keeps the seam lined up; the default span zoom is
  coarsest pitch / own pitch, so the picture is the same physical size
  everywhere. `ui.html` mirrors all of this in JS (`pictureViews`,
  `placePicture`); keep them in step. `Backdrop::layered` is true
  unless it's a plain colour.
- `src/edid.rs`: a monitor's size in mm from its EDID (first detailed
  timing, else the basic cm size) and the registry key it's under.
- `src/config.rs`: parses and validates `config.toml` (serde + toml)
  into `Settings`: background (`"#RRGGBB"` or `"transparent"`),
  `image` / `image_fit` (each layout may override all three;
  `image = ""` turns the picture off; `backdrop_for` resolves them;
  relative picture paths are made relative to the config's folder by
  `with_base_dir`), and layouts matched to monitors by
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
  offline icon. It records hit areas while drawing. The VOICE page has
  a names / avatars-only toggle (icon at the end of the channel line).
  The widget is drawn `BLEED` units in from each side so rings and
  badges at its edges aren't cut off.
- `src/image.rs`: decoded pictures (premultiplied BGRA).
- `src/audio.rs`: sound to bars: sample formats to mono, the latest
  4096 samples, and an iterative FFT into 32 log-spaced bands (20 Hz
  to 20 kHz, with a 3 dB/octave tilt; 4096 so the bass bands get
  ~12 Hz bins). Bars are shaded green (quiet, bottom) through yellow
  and orange to red (loud, top), with frequency labels underneath. `src/widgets/visualizer.rs`
  draws them like the wallpaper's (0.7vh bars, 1.104vh apart, 7vh
  tall), rising fast and falling 5% a frame.
- `src/win32/`: the Win32 layer.
  - `desktop.rs`: finds where to draw. Explorer's split desktop has two
    shapes: **Raised** (Windows 11 24H2+: surfaces are layered children
    of `Progman`, directly below `SHELLDLL_DefView`) and **Classic**
    (surfaces inside the wallpaper `WorkerW`). Undocumented; message
    `0x052C` with wParam `0xD` asks Explorer to split.
  - `app.rs`: hidden controller window and message loop. A sensor
    tick every 0.5 s: the CPU and network every tick (repainting only
    their blocks), the GPUs, RAM, SSDs and a config-file change check
    every other tick. The CPU and GPU counters are separate PDH
    queries because the GPU engine counter (one instance per process
    per engine) is the expensive one. Health check every 5 s; rebuilds
    on config change, Explorer restart (`TaskbarCreated`), display change
    and resume. A bad config is logged and the old settings kept.
  - Layered backdrops (transparent or a picture) don't draw the
    widgets into the surface. `layer.rs`: each widget gets its own
    per-pixel-alpha window (`UpdateLayeredWindowIndirect`, from the
    widget's DIB, only the changed rectangle), inside a widget-sized
    colour-keyed *frame* that's a child of the desktop host. The canvas
    keeps premultiplied alpha (`Canvas::clear`), and `paint.rs` draws
    labels white on black into a small mask (greyscale font quality)
    and blends them in. The surface is then alpha 0 (transparent) or
    the colour, painted once; a picture gets its own window
    (`PictureWindow`), sized to where the picture goes and painted once
    after it's shown, by `wallpaper.rs` (WIC, scaled and drawn in 1 MB
    bands, so no monitor-sized copy is ever held). Z-order: icons,
    widget frames, picture, surface. What was learned the hard way
    (24H2, Raised): per-pixel-alpha children of `Progman` never show
    (the call succeeds) but do inside a colour-keyed child; Windows
    only takes per-pixel pixels from a buffer the size of the whole
    window; a colour-keyed window costs a copy of its pixels in our
    working set (66 MB for both monitors), so never key a monitor-sized
    one; a parent at alpha 0 hides its children; a layered window only
    keeps GDI painting done while it's visible.
  - `surface.rs`: one window per monitor. `paint.rs`: GDI background
    plus the layout's widgets, each with its own persistent DIB section:
    the canvas draws straight into it, GDI adds the text (Bahnschrift,
    built into Windows; the wallpaper's own font isn't ours to ship) and
    BitBlts it in. Only the widgets' areas are repainted each second,
    and `WM_PAINT` redraws just the rectangles in the window's update
    region (clipped canvas + clipped text), so glide frames redraw only
    the moving parts. `app.rs` runs a 33 ms frame timer only while a
    glide is under way.
    Scale = monitor height / 1000 (at least 1 px per unit) x layout size.
  - `monitors.rs`: each monitor's rectangle, work area, name, size in
    mm (EDID from the registry, via `EnumDisplayDevicesW` with
    `EDD_GET_DEVICE_INTERFACE_NAME`) and primary
    flag.
  - `discord/`: the session thread plus a pipe-reader thread
    (overlapped I/O on `\\.\pipe\discord-ipc-N`), the bot thread plus a
    websocket-reader thread (`gateway.rs`), and `focus.rs` (raise
    Discord's window after opening a `discord://` link).
  - `audio.rs`: WASAPI loopback capture of the default speakers on its
    own thread (event-driven, so it sleeps in silence), reopened when
    the default device changes. It posts `WM_AUDIO` when sound starts;
    `app.rs` then runs a 33 ms visualizer timer until the sound stops
    and the bars are down, and rests it while
    `SHQueryUserNotificationState` reports a full-screen app. Capture
    only runs while a layout has a visualizer.
  - `media.rs` (+ safe `src/media.rs`): now playing. A thread polls
    Windows' media sessions (WinRT `GlobalSystemMediaTransportControls`)
    every 2 s, picks one (playing > paused; music apps > browsers >
    others), and on a new song decodes its thumbnail (WIC via
    `CreateStreamOverRandomAccessStream`, cropped square). It posts
    `WM_MEDIA`; the visualizer shows art left of the bars and the title
    (scrolling in a clipped box when long) and artist underneath.
  - Sleep (`check_cover` in `app.rs`): in Light, a monitor whose
    middle shows an app window covering all of it (`WindowFromPoint`)
    rests: `Surface::rest` frees its widget buffers and fonts, and it
    ignores repaints, keeping what's on screen. It wakes once
    uncovered, at any level. While every monitor with widgets rests,
    the app sleeps: sensor, frame and visualizer timers stop, the audio
    and media threads end, pictures are dropped, the working set is
    trimmed, and a 1 s wake timer checks for a widget to show again.
  - The visualizer rests only while the foreground window covers a
    monitor showing it (`Surface::visualizer_hidden_by`), so full-screen
    video on another monitor doesn't stop it.
  - `[panel]` (`Parts`): which parts the panel, `gpus` and `info` widgets
    show; `Dashboard::panel_layout` reflows them. `[network] max_mbps`
    plus the adapter's link speed set where the graph's colours (the
    visualizer's green-to-red scale) reach red.
  - `http.rs`: WinHTTP requests and websockets (no TLS library).
    `images.rs`: download + WIC decode on a thread, with retries.
  - `tray.rs`: the notification-area icon (the exe's icon resource at
    the small-icon size) and its menu ("Open settings" starts
    `wallflex-settings.exe` if installed); `app.rs` handles the choices and
    re-adds the icon when Explorer restarts. `startup.rs`: "Start with
    Windows" as an HKCU `Run` value.
  - `mouse.rs`: the WH_MOUSE_LL hook runs on its own thread that only
    pumps messages. Windows waits for low-level hooks before moving the
    cursor, so a hook on the painting thread lagged the mouse badly
    whenever frames were busy (seen with full-screen YouTube and the
    visualizer at ~60 fps). Never move it back to the UI thread.
  - Clicks: a WH_MOUSE_LL hook (only while Discord is set up) notes a
    press and release at the same spot, and only acts when the desktop
    itself is under the pointer. It never swallows clicks. Pointer
    moves only matter inside the Discord widget's rectangles (cached
    after each paint): there, one `WM_DESKTOP_HOVER` at a time carries
    the position, and the widget repaints only when the hovered control
    changes (notes, the view toggle, SERVER buttons and list).
  - Discord text goes through `model::clean_text` (via `fit`): GDI draws
    invisible characters such as Discord's direction isolates as boxes,
    and has no colour emoji (they draw in one colour).
  - `sensors/`: raw readings, all from built-in Windows sources with no
    admin: `pdh.rs` (CPU per core and clock, GPU engine use, VRAM in
    use), `gpu.rs` (D3DKMT: temperature, fan, power, VRAM size),
    `net.rs` (interface byte counters), `disk.rs` (SSD temperature
    query; hard drives skipped so they never spin up). Ported from the
    user's own `windows_sensors.py`.
- `wallflex.manifest`: per-monitor DPI v2 and Windows 8+ support (needed
  for layered child windows); embedded by `build.rs`.
- `wallflex.ico`: the app icon (exe, tray, installer), drawn by
  `src/icon.rs` (test-only) at 16 to 128 px. A test fails if it no
  longer matches the drawing; `WALLFLEX_WRITE_ICON=1 cargo test icon`
  rewrites it. `build.rs` packs it into a `.res` file (resource id 1)
  for the linker, so no resource compiler is needed.

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
  `--widget all` treats `<file>` as a folder and saves every widget
  (`panel.bmp`, ..., `gpu1.bmp`; Discord without avatars).
- Discord credentials and the bot token live in `[discord]` in the
  config; the OAuth token is `discord-token.json` next to it. Never
  print or commit them.
- `docs/screenshot.png` (in the README) is composed from `--preview 1440`
  renders of `panel`, `discord` and `visualizer` (sample people and
  song, so no real Discord data): panel and Discord on top, visualizer
  centred below, black, 48 px margins. Never use a capture of the
  user's desktop; it shows their friends, servers and media.
- Versioning: SemVer, tags `vX.Y.Z`; before 1.0, minor = features or
  config changes, patch = fixes. Note user-visible changes under
  `## [Unreleased]` in `CHANGELOG.md` as they're made. `tools\release.ps1`
  publishes the version in `Cargo.toml` (checks, package, tag, GitHub
  release with the changelog's notes).
- `tools\package.ps1` builds `dist\wallflex-<version>.zip` (exe,
  `docs/SETUP.md`, the default config, licences) and the installer
  `dist\wallflex-<version>-setup.exe` from `installer\wallflex.iss`
  (Inno Setup 6, per user, no admin, into `%LOCALAPPDATA%\Wallflex`;
  ISCC is in `%LOCALAPPDATA%\Programs\Inno Setup 6`). Never change its
  `AppId`. Test it with `/VERYSILENT /DIR=<scratch>`: its install and
  uninstall run `--quit`, which closes the user's running copy, and the
  startup task rewrites their `Run` value, so save and restore both.
  `docs/SETUP.md` is the friend-facing guide: keep it in step with the
  config, menu and installer.
- Test layouts with `--config <file>` instead of editing the user's
  real `%APPDATA%\Wallflex\config.toml`; the log goes next to that
  file.
- The user's own copy is installed by the setup into
  `%LOCALAPPDATA%\Wallflex` (started at sign-in via the HKCU Run key);
  update it by running a newer `dist\wallflex-<version>-setup.exe`,
  which quits the running copy first. The old setup (DiscordBridge
  task, PerformanceMonitor and Wallpaper Engine services, WEAdapterFix)
  is disabled; its backups have been deleted.
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

After milestone 6 (23 Oct 2026, release build, 60 s, panel on both
monitors, no Discord): repainting everything 30 times a second cost
0.79% CPU (60: 1.7%) against 0.17% at once a second, with no RAM
change. With the 0.3 s glide (about 9 frames per reading) and
update-region repaints: 0.28% CPU, 18.7 MB working set. The RAM bar
has a section per stick, counted from SMBIOS type 17 entries.

CPU and network twice a second (23 Oct 2026, same setup): 0.31% CPU
against 0.20% for everything once a second, measured back to back.
Reading everything twice a second cost 0.82%.

Visualizer (24 Oct 2026, release build, 45 s runs, PC locked so the
numbers are noisy): panel only 0.43-0.49% CPU; panel + visualizer in
silence 0.40% (no frames, audio thread asleep); panel + a test build
feeding fake music every frame 0.34-0.58%, so the bars cost about 0.1%
or less. Memory +3.5 MB working set, +1 MB private (Windows audio DLLs,
3 more threads). Not yet watched live with real music.

Where animation time goes (24 Oct 2026, release build, 4K at size
0.9, panel + visualizer + Discord, music playing): 130-250 ms of
main-thread time a second, almost all of it `WM_PAINT`. A shared
timer for the glide and visualizer was tried and dropped: it halved
the paints (~125 to ~68 a second) but not the time, measured back to
back, since the cost is pixels drawn, not paints. Per part of a glide
frame (`cargo test --release bench_glide -- --ignored --nocapture`):
CPU columns 315 us, network graph 113, each GPU's rings ~200, RAM
bar 72; an empty-clip draw is 2 us, so it's pixel work, ~7 ns a
pixel. Skipping panel blocks outside the clip (`Pen::reaches`) gave
9%, and writing opaque fully-covered pixels straight in
(`Canvas::fill_rect`) took the CPU columns to 236 us: 965 to 820 us a
frame overall. Then rectangles were snapped to whole pixels
(`snap_area`; line weights rounded to whole pixels) so they have no
partly covered pixels, and `fill_rect` reuses its last blend when the
pixel underneath and the strength repeat (translucent outlines over
the background): CPU columns 57 us, RAM bar 11, 572 us a frame. Edges
round independently, so at 1080 columns and gaps can differ by a
pixel. What's left: the rings' arcs (~200 us per GPU, smoothed by
nature, ~16 fps) and the network graph (~100 us, ~32 fps).

CPU spikes that weren't (24 Oct 2026): Task Manager and the governor
showed Wallflex jumping to 3-5% every few minutes, and the governor
kept stepping down to normal for 10-20 s. Per-thread times said the
main thread, but no message handler, sent or posted, was busy then.
`GetProcessTimes`/`GetThreadTimes` (and Task Manager) charge a whole
15.6 ms clock tick to the thread running when it lands, and the frame
timers wake Wallflex right on the ticks. Side by side with
`QueryThreadCycleTime`: busy seconds read 270-390 ms sampled against
92-102 ms of cycles. `CpuMeter` now divides `QueryProcessCycleTime` by
the timestamp counter (`_rdtsc`; Windows counts thread cycles in its
units). To measure Wallflex from outside, use cycle counts (Process
Explorer) rather than Task Manager's CPU column.

Transparent and picture backgrounds (24 Oct 2026, release build, 45 s,
panel + visualizer on the 4K monitor, music playing): colour 32.2 MB
working set, 12.3 MB private, 0.60% CPU; transparent 37 MB, 12.1 MB
private, 0.38% CPU (noisy; no worse). A picture adds almost nothing
once painted (peak private while decoding stayed under 13 MB).

Smoother smooth mode (24 Oct 2026): CPU and network four times a
second, CPU glides lasting until the next reading, ring/RAM glides a
full second, and the network graph sliding between readings with its
scale easing. First try cost 3.4-3.9% CPU once the graph filled (the
governor kept stepping down): `Canvas::polyline` visits the line's
whole bounding box, so the graph now uses `graph_line` (per column);
and each group repaints at its own rate (`Pace`: columns ~64 fps,
graph ~32, rings/RAM ~16) and only while it moves. Result: 1.02% CPU
over 75 s with a full graph, no step-downs. Profile repaints by timing
`render` per part when this needs revisiting.
