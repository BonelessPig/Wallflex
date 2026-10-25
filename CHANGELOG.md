# Changelog

All notable changes to Wallflex. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions
follow [Semantic Versioning](https://semver.org/): before 1.0, the minor
number (0.**x**.0) goes up for new features or changed settings and the
patch number (0.x.**y**) for fixes only.

## [Unreleased]

### Changed

- Smooth mode no longer steps down to normal or light on its own: it
  stays smooth until the PC's CPU or RAM is 90% used (was CPU at 80%,
  or Wallflex itself over 2% CPU), then drops to light and climbs back
  once the PC has had room for a while.

## [0.3.0] - 2026-10-24

### Added

- `background = "transparent"` keeps your Windows wallpaper and draws
  just the widgets on top, with smooth edges and text.
- `image` shows a picture behind the widgets (JPEG, PNG, BMP, GIF or
  TIFF), fitted with `image_fit`: `"fill"`, `"fit"`, `"stretch"` or
  `"center"`. Set it once for every monitor, or per layout for a
  different picture (or none) on each. The settings app has both under
  **General**, and per layout.

## [0.2.0] - 2026-10-24

### Added

- An installer, `wallflex-<version>-setup.exe`: installs for your
  account only (no admin), adds a Start menu entry and optionally
  starts Wallflex at sign-in. Upgrades close the running copy and keep
  your settings; it uninstalls from Installed apps and asks before
  deleting your settings. The zip is still there for a portable copy.
- An app icon (the tray icon's three bars on blue) for the exe, the
  Start menu, Installed apps and the installer.
- `[network] seconds` sets how much history the network graph shows
  (10 to 300).
- An optional settings app, `wallflex-settings.exe` (**Open settings**
  in the tray menu, or **Wallflex Settings** in the Start menu): your
  monitors with live previews of the widgets, which you drag into
  place, and tabs for layouts, panel parts, the network graph, Discord,
  background and performance. It checks changes with Wallflex before
  saving and keeps the old file as `config.toml.bak`. The installer
  offers it as an optional component.
- `wallflex --check` checks the settings file, `wallflex --info`
  prints the monitors and GPUs as JSON, and `--preview` with
  `--widget all` saves every widget. With `--config`, previews use
  that file's panel parts, graph length and background.

### Changed

- The network graph shows the last 30 seconds by default (was a
  minute), with rounded lines and filled areas under download and
  upload.
- Animation costs less: a frame of the gliding bars no longer redraws
  the panel's other blocks, solid fills skip the blending maths, and
  Discord updates repaint only the Discord widget.
- Bars, columns, outlines and other rectangles are drawn on whole
  pixels, so their edges are crisp instead of smoothed, and repeated
  blends are reused. With the change above, a 4K glide frame went
  from about 0.97 ms to 0.57 ms. Rings and curves are still smoothed.
- Smooth mode now stays smooth until the PC is 80% busy, then drops
  straight to light (it eased off at 60% and again at 85%).
- In light mode, a monitor hidden by a full-screen window stops
  drawing and frees its memory. When every monitor with widgets is
  hidden, Wallflex sleeps (no sensor readings, sound capture or
  animation) until one is visible again.

### Fixed

- Wallflex no longer keeps switching itself from smooth to normal for
  no reason. It measured its own CPU use with Windows' usual counters,
  which charge a whole 15.6 ms clock tick to whatever is running when
  the tick lands; with animation timers waking it right on the ticks,
  that read three to four times the real use. It now counts CPU
  cycles. (Task Manager uses the same tick counters, so it still shows
  short 3-5% jumps that aren't real.)

## [0.1.0] - 2026-10-24

The first release.

### Added

- Widgets drawn behind the desktop icons, on every monitor, with a
  layout per monitor (anchored to corners or edges, sized to the
  monitor's height) in `%APPDATA%\Wallflex\config.toml`, reloaded live.
- Performance panel: CPU thread columns grouped by real core, network
  graph shaded green to red toward the adapter's (or your plan's)
  speed, per-GPU rings (load, VRAM, temperature, power, fan), RAM bar
  with a section per memory stick, SSD temperatures. `[panel]` turns
  parts off; by default everything detected is shown.
- Discord widget (your own Discord application): your call with
  speaking rings and mute/deafen badges, names or avatars-only view,
  a server's voice channels, friends online, recent notifications that
  open Discord when clicked, hover highlights.
- Music visualizer: 32 bands from 20 Hz to 20 kHz, coloured by
  loudness, with what's playing (album art or video thumbnail, title,
  artist) from Windows' media controls.
- `performance = "smooth"` (default) or `"light"`, with Wallflex keeping
  itself under 2% CPU and 120 MB of memory either way.
- Tray menu (settings, log, start with Windows, quit), `--preview` to
  render widgets to a BMP, and a setup guide for friends.

[Unreleased]: https://github.com/BonelessPig/Wallflex/compare/v0.3.0...HEAD
[0.3.0]: https://github.com/BonelessPig/Wallflex/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/BonelessPig/Wallflex/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/BonelessPig/Wallflex/releases/tag/v0.1.0
