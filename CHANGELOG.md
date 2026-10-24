# Changelog

All notable changes to Wallflex. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions
follow [Semantic Versioning](https://semver.org/): before 1.0, the minor
number (0.**x**.0) goes up for new features or changed settings and the
patch number (0.x.**y**) for fixes only.

## [Unreleased]

### Added

- An installer, `wallflex-<version>-setup.exe`: installs for your
  account only (no admin), adds a Start menu entry and optionally
  starts Wallflex at sign-in. Upgrades close the running copy and keep
  your settings; it uninstalls from Installed apps and asks before
  deleting your settings. The zip is still there for a portable copy.

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

[Unreleased]: https://github.com/BonelessPig/Wallflex/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/BonelessPig/Wallflex/releases/tag/v0.1.0
