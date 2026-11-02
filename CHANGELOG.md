# Changelog

All notable changes to Wallflex. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions
follow [Semantic Versioning](https://semver.org/): before 1.0, the minor
number (0.**x**.0) goes up for new features or changed settings and the
patch number (0.x.**y**) for fixes only.

## [Unreleased]

### Added

- The reader's guide list has a Claude entry that shows claude.ai in a
  second browser view (sign in once; it's kept with the reader's other
  browser data). The Contents column then lists Claude's sidebar: New
  chat, Chats, Projects and Artifacts. Leaving Claude closes the view.
  Sign in with email; Google refuses sign-in from embedded browsers.
  Claude takes the reader's style: light or dark to suit the theme, the
  theme's page and text colours (labels and icons too), and the reading
  font, size and line spacing for messages and code. On light themes
  code is darkened, since Claude colours it for the account's own mode.
  Claude's other text (thinking steps, labels, the reply box) takes the
  reading font and is scaled to match the reading size, with its
  padding and spacing (in Claude's compact layout). Contents shows or
  hides the sidebar list; hidden, Claude's own sidebar has room to
  show. The corner menu (with its fading backdrop),
  guide list, Aa panel and contents rings open over Claude, as they do
  over guides.
- `tools\package.ps1` checks the reader's and settings pages' scripts
  with `node --check` (a syntax error blanks the whole page).

- The reader draws maths in Markdown guides (`$x^2$`, `$$...$$`), so
  converted LaTeX shows its formulas instead of raw TeX. Unreadable
  formulas stay as their source.
- The reader can show web pages in reader mode (an article without the
  menus and ads, in your reading style). Open the guide list (Ctrl+K),
  paste an address such as `oldschool.runescape.wiki/w/Abyssal_whip`
  and press Enter. Links in the page open in reader mode too, with a
  Back link and "Open in browser" at the top. Pages are fetched with
  Windows' own `curl.exe`; scripts never run.
- A web page in the reader has a **Save as guide** link that writes it
  into the Guides folder as Markdown (never overwriting; the source
  address is noted at the top). Markdown guides may now show pictures
  from https addresses.
- A **Contents** button in the reader's corner menu. Clicking it pins
  the page's headings as a column in the left margin (clicking again
  unpins); the column follows your place, shows a title too long for it
  in full when you point at it, and is remembered. Narrow windows hide
  the column. Hovering the button instead fans the main sections out
  around the corner; pointing at one with sub-sections opens a further
  ring for them, and so on (rings above the parent's keep only the
  section that led there; nothing moves as you go deeper). Click a section to
  jump to it. It works for
  web pages and for Markdown and text guides.
  Its text is 1 px bigger than the reading text size you set, and its
  other sizes follow (its rows are tighter in Compact line spacing and
  looser in Airy).
  Drag the column's right edge to resize it (double-click for the usual
  width); the width is remembered.
- In the pinned contents column, sections that have sub-sections get an
  arrow to fold or unfold them. Folded sections stay folded while you
  read the page, and the section holding your place is marked.
  Buttons at the top of the column fold or unfold every section at once.
- The reader's guide list has a **-> MD** button on HTML and LaTeX
  guides that converts them to Markdown with pandoc (found on `PATH`;
  not included), writing a `.md` copy beside the original. LaTeX
  (`.tex`) files now show in the guide list as plain text. PDFs can't be
  converted.
- Dragging the floating reader against the left or right edge of the
  screen and letting go pins it to that side.
- Dragging the reader's menu dot while it is pinned unpins it and floats
  the window with the dot under the pointer.
- The reader's settings (gear) panel has a **Menu dot** choice that moves the dot
  (and its fan of buttons) to any corner. A bottom corner keeps it off
  the toolbar of Edge's PDF viewer, which it covered at top-left.
- A `guides` widget: a book icon that opens your own guides in a
  borderless reader window on the desktop. Put Markdown, HTML, PDF or
  text files in the `Guides` folder next to `config.toml`
  (`%APPDATA%\Wallflex\Guides`). The reader floats in the corner by the
  icon and hides when you click elsewhere, press Esc or click the icon
  again; its pin buttons dock it full height to the left or right side
  of the screen instead (the side is remembered), and maximised windows
  make room for it. Ctrl+K picks a guide.
  It's a separate, optional program (`wallflex-reader.exe`) using
  WebView2, so it only uses memory while it's open, and it exits two
  minutes after it's hidden.
- The reader's settings (gear) button sets how guides look: a theme (Auto, which
  follows Windows, Light, Sepia, Dark or Black), the font (Segoe UI,
  Sitka, Georgia, Verdana, Bahnschrift or Cascadia Mono), the text size
  and the line spacing. Changes show at once and are remembered.
  Markdown and text guides follow all of it; HTML and PDF guides keep
  their own look.
- The reader has no title bar: the guide fills the window, and its
  buttons (guides, settings, folder, pin left, pin right, close) fan out
  from a faint dot in the top-left corner when you point at it.
- The reader can be resized: drag any edge or corner of the floating
  window, or the inner edge of the pinned one. Both sizes are
  remembered, and a pinned reader's new width is kept free for it.
- The floating reader can be moved: drag the dot in its top-left
  corner (a click still opens the buttons). It stays where you put it,
  also next time, unless you open it from a book icon on another
  monitor.

### Removed

- The reader's two pin buttons (dock left, dock right) are gone from
  the corner menu. Dragging the window against a side of the screen
  still docks it, and dragging the menu dot undocks it. The Contents
  button no longer shows a caption.
- The contents list's Unpin button. Press the Contents button in the
  corner menu again to unpin it.

### Fixed

- Widgets move to stay clear when the work area changes, such as when
  the guides reader is pinned to a side or the taskbar moves; before,
  they stayed put until Wallflex restarted.
- The installer now closes a running guides reader before upgrading or
  uninstalling; before, it only quit Wallflex and the reader blocked
  the update. `wallflex-reader --quit` closes a running reader.

## [0.5.0] - 2026-10-25

### Added

- A picture library in the settings app: **Browse…** next to a picture
  box opens a gallery of the pictures in `%APPDATA%\Wallflex\Pictures`,
  grouped by folder, to pick from. **Import files…**, **Import
  folder…** or dragging pictures in copies them there (the originals
  stay put, and the same picture isn't copied twice). Pictures can be
  deleted from the gallery, and **Open folder** shows the library in
  File Explorer.

## [0.4.0] - 2026-10-25

### Added

- `image_fit = "span"` stretches one picture across every monitor that
  uses it, drawn the same physical size on each (from the size the
  monitor reports), so it carries on across the gap between screens.
- Per layout, `image_zoom` and `image_offset = [x, y]` zoom and move
  its monitor's picture, to line a spanned picture up exactly (or to
  frame any picture).
- The settings app shows the picture itself on each monitor, has
  **Zoom** and **Shift** per layout, moves a picture with Alt+drag,
  and can draw the monitors at their real sizes.
- `wallflex --info` gives each monitor's size in millimetres.

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

[Unreleased]: https://github.com/BonelessPig/Wallflex/compare/v0.5.0...HEAD
[0.5.0]: https://github.com/BonelessPig/Wallflex/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/BonelessPig/Wallflex/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/BonelessPig/Wallflex/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/BonelessPig/Wallflex/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/BonelessPig/Wallflex/releases/tag/v0.1.0
