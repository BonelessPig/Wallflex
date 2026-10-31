# Porting Wallflex to macOS and Linux

Where things stand, what each system needs, and in what order. Written
31 Oct 2026 alongside the groundwork on the `port/groundwork` branch.

## Summary

- About two thirds of the code (~17,500 of ~26,000 lines) is already
  portable: the widgets, the canvas, config, backgrounds, the Discord
  protocol, the audio maths, pacing and the settings server.
- The Windows layer, `src/win32/` (~8,800 lines), has to be written
  again for each system. Nothing in it carries over except the shape of
  the code.
- **macOS** has one desktop and a supported way to draw at desktop
  level, so the core of a port is straightforward. The hard parts are
  in the extras: system audio for the visualizer, Now Playing and
  temperatures.
- **Linux** is the reverse. The readings, audio and Now Playing are
  easy, but drawing behind the icons works differently on each desktop
  (and isn't possible on GNOME under Wayland without a shell
  extension).
- Each port is roughly 6,000-9,000 lines of new platform code, about
  the size of the Windows layer. macOS comes first.

## What the groundwork did

- `src/platform.rs` is now the only place `main.rs` reaches the system.
  It re-exports the same few items from `win32` (Windows), `macos` or
  `linux`. On Windows nothing changed: the previews are byte-for-byte
  identical.
- The crate builds, passes clippy and runs its tests on macOS and Linux.
  CI does all three. `--sensors`, `--preview`, `--check` and `--info`
  work there; running the app says it isn't supported yet.
- `--preview` on macOS/Linux draws the canvas as on Windows, with text
  from `fontdue` (`src/font.rs`) and a font the system already has
  (`WALLFLEX_FONT` overrides it).
- macOS readings: CPU per core (Mach tick counts), RAM (as Activity
  Monitor counts "Memory Used") and network (`getifaddrs`, `en*`
  interfaces). The maths is in `sensors::counters`, which is tested on
  every system.
- The settings path is per system (`src/paths.rs`):
  `~/Library/Application Support/Wallflex` and `~/.config/wallflex`.
  The settings app finds `wallflex` without `.exe`, and opens things
  with `open` / `xdg-open`. Picture paths saved Windows-style
  (`Pictures\sky.jpg`) are read with `/` elsewhere.

## Part by part

Effort is rough: S = a day or less, M = a few days, L = a week or more.

| Part | Windows today | macOS | Linux |
| --- | --- | --- | --- |
| Behind the icons | Undocumented `Progman`/`WorkerW` split | **M.** A borderless `NSWindow` at `kCGDesktopWindowLevel`, on all Spaces, ignoring clicks except on widgets. Supported, and it's how Übersicht and Plash work. | **L.** X11: `_NET_WM_WINDOW_TYPE_DESKTOP` (or below). Wayland: `wlr-layer-shell` `background`/`bottom` (KDE, Hyprland, Sway). GNOME Wayland has no such protocol, so it needs a shell extension. On KDE the widgets may sit above the desktop icons. |
| Per-pixel transparency | Layered windows, many workarounds | **S.** A transparent window with a `CALayer` of our pixels. | **M.** ARGB visual (X11) or `wl_shm` buffers (Wayland). |
| Text | GDI, Bahnschrift | Done for previews (`fontdue`). Could stay that way, or use Core Text for the system font. | Done for previews (`fontdue`). |
| CPU / RAM / network | PDH, `GlobalMemoryStatusEx`, `GetIfTable2` | **Done** (no clock yet; Apple Silicon has no public clock reading) | **S.** `/proc/stat`, `/proc/meminfo`, `/sys/class/net/*/statistics`, `/sys/devices/system/cpu/*/topology` |
| GPU use / VRAM | PDH engine counters, D3DKMT | **M.** IOKit `IOAccelerator` `PerformanceStatistics` ("Device Utilization %"), no root needed | **M.** AMD: `gpu_busy_percent` / `mem_info_vram_*` in sysfs. NVIDIA: NVML via `dlopen`. Intel: little without root. |
| Temperatures | D3DKMT, storage query | **L.** SMC keys via IOKit; Apple Silicon needs the undocumented IOHID sensor calls | **S.** `/sys/class/hwmon` (`amdgpu`, `nvme`, `coretemp`/`k10temp`) |
| Monitors (`--info`) | `EnumDisplayMonitors` + EDID | **S.** `CGGetActiveDisplayList`, `CGDisplayBounds`, `CGDisplayScreenSize` gives mm directly; `NSScreen.visibleFrame` for the work area | **M.** `wl_output` / XRandR; EDID from `/sys/class/drm/*/edid` (parser exists in `edid.rs`) |
| System audio (visualizer) | WASAPI loopback | **L.** Core Audio process taps (macOS 14.2+), behind a permission prompt; older systems need a virtual audio device | **S.** PipeWire or PulseAudio monitor source |
| Now Playing | WinRT media sessions | **L, risky.** MediaRemote is undocumented, and since 15.4 it refuses apps without Apple's entitlement. The known workaround runs a helper through `/usr/bin/perl`. Could fall back to AppleScript for Music and Spotify. | **S.** MPRIS over D-Bus |
| Tray | `Shell_NotifyIcon` | **S.** `NSStatusItem` (menu bar) | **M.** StatusNotifierItem over D-Bus. GNOME needs the AppIndicator extension. |
| Start at login | HKCU `Run` | **S.** `SMAppService` (13+) or a LaunchAgent plist | **S.** `~/.config/autostart/wallflex.desktop` |
| Clicks on widgets | Low-level mouse hook on its own thread | **S.** The window takes clicks where the widgets are (`ignoresMouseEvents` toggled by hit test) | **S/M.** Layer-shell or X11 window input region |
| Covered / full screen | `WindowFromPoint`, `SHQueryUserNotificationState` | **M.** `CGWindowListCopyWindowInfo` needs Screen Recording permission for window names, but bounds work without it; occlusion via `NSWindow.occlusionState` | **M.** Per compositor; layer-shell has no occlusion events |
| HTTPS / websockets | WinHTTP | **M.** `rustls` + a small websocket client, or `NSURLSession` | **M.** `rustls` + a small websocket client |
| Image decoding | WIC | **S.** ImageIO (`CGImageSource`) | **M.** A decoder crate (`png`, `jpeg-decoder`, `image-webp`) |
| Discord pipe | `\\.\pipe\discord-ipc-N` | **S.** Unix socket `$TMPDIR/discord-ipc-N` | **S.** `$XDG_RUNTIME_DIR/discord-ipc-N`, plus the Flatpak and Snap paths |
| Raise Discord after a link | `focus.rs` | **S.** `open discord://...` already raises it | **S.** `xdg-open` |
| Settings window | Edge `--app` | **S.** Chrome/Edge `--app` if installed, else `open` | **S.** Chromium `--app` if installed, else `xdg-open` |
| Guides reader (in progress on `main`) | WebView2 window | **M.** `WKWebView` | **M.** WebKitGTK |
| Packaging | Inno Setup | **M.** `.app` bundle, signed and notarised (needs a paid Apple developer account), as a DMG or zip | **M.** AppImage or `.deb`; Flatpak can't reach the desktop layer |

## Risks

- **RAM budget.** AppKit's own baseline is probably 20-30 MB, above the
  15 MB target before any widget is drawn. Measure the empty window
  first (step 1 below) and decide whether the target changes on macOS.
- **macOS permissions.** System audio and window names each bring a
  prompt. Without a signed build, the system asks again after every
  rebuild.
- **Now Playing on macOS** may not be possible in a supported way. Plan
  for the visualizer to show bars only.
- **GNOME on Wayland** (Ubuntu's default) can't place a window behind
  the icons without a shell extension. Wallflex would need one (a
  separate JavaScript project), or that desktop goes unsupported.
- **Testing.** There's no Mac or Linux machine here. CI runs the tests
  and previews, but the desktop window needs real hardware to check.

## Recommended order (macOS)

1. A desktop-level window on each screen, drawing the panel from a
   reading (no glides). Measure RAM and CPU against the budget straight
   away.
2. Monitors (`--info`), live reload of the config, and repainting only
   what changed (the `Pace` timers from `src/pace.rs`).
3. Menu-bar item and start at login. This is the first build a friend
   could use.
4. GPU use (IOKit), then temperatures.
5. Discord: the Unix socket transport for `rpc.rs`, HTTPS and images.
6. Clicks on the Discord widget.
7. The visualizer (process tap) and, if a workable route exists, Now
   Playing.
8. A signed and notarised `.app`.

Linux follows the same order, starting with `wlr-layer-shell` and X11.
Its readings, audio and media steps are each much smaller.

## How the code is shaped for it

- Add each system's layer next to `win32/`. Only that tree may use
  `unsafe`, and every block needs a `// SAFETY:` comment, as now.
- Everything `main.rs` needs goes through `platform.rs`. When the macOS
  layer gets `app::run`, it replaces the "not supported yet" stub there.
- Keep maths out of the layers (as `sensors::counters` does), so it's
  tested on every system, including the Windows machine this is built on.
- `#![cfg_attr(not(windows), allow(dead_code))]` in `main.rs` hides
  shared code that has no caller yet off Windows. Remove it once the
  macOS app runs.
