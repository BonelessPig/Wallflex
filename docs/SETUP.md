# Setting up Wallflex

Wallflex draws desktop widgets (CPU, network, GPU, RAM, SSDs and an
optional Discord panel) behind your desktop icons. It's one small exe:
no admin rights, no Wallpaper Engine.

Needs Windows 10 or 11.

## 1. Install

Run `wallflex-<version>-setup.exe`. It installs for your account only
(no admin) into `%LOCALAPPDATA%\Wallflex`, adds Wallflex to the Start
menu, and can start it when you sign in. Windows SmartScreen may warn
that it's from an unknown publisher, because it isn't signed: choose
**More info > Run anyway**. To update, run a newer setup: it closes
Wallflex, replaces it and keeps your settings.

Or, for a portable copy, unzip `wallflex-<version>.zip` somewhere
permanent and double-click `wallflex.exe`. (If you move the exe later,
turn "Start with Windows" off and on again so it points at the new
place.)

The widgets appear behind your icons, and a Wallflex icon appears in
the notification area. Windows 11 hides new icons under the **^**
button at first; drag it onto the taskbar to keep it in sight.

## 2. The tray menu

Click or right-click the Wallflex icon:

- **Open settings** opens `config.toml` (in Notepad if nothing else is
  set up for `.toml` files).
- **Open log** opens `wallflex.log`, which explains any problem.
- **Start with Windows** starts Wallflex when you sign in. Click again
  to stop that.
- **Quit Wallflex** removes the widgets until you start it again.

## 3. Settings

The first run writes `%APPDATA%\Wallflex\config.toml`. Every option is
explained in comments inside it (`config.example.toml` in the zip is
the same file). Save it and the change shows about a second later.

- `background`: the colour behind the widgets, such as `"#101418"`.
- `performance`: `"smooth"` (the default) for smooth animations while
  your PC has room, or `"light"` for no animations and the least work.
  Either way Wallflex stays under 2% CPU and 120 MB of memory.
- `[[layout]]`: which widgets go on which monitor. `monitor` can be
  `"*"` (every monitor), `"primary"`, a resolution such as
  `"2560x1440"`, or a name such as `"DISPLAY2"` (the log lists your
  monitors' names when Wallflex starts).
- `[[layout.widget]]`: a widget's `kind`, the corner or edge it sits
  in, and how far from it. Distances are in thousandths of the
  monitor's height, so a layout looks the same at any resolution.

If you make a mistake, Wallflex keeps the last good settings and says
what's wrong in the log.

For a music visualizer, add a widget with `kind = "visualizer"`. It
shows whatever your default speakers play, with the song or video's
title, artist and album art (from Windows' media controls, preferring
music apps, then browsers). It uses no CPU while it's quiet, and rests
while a full-screen window covers its monitor.

`[panel]` hides parts you don't want (`cpu`, `network`, `ram`, `ssd`,
or `gpus = [1]` for only the first GPU); by default everything this PC
has is shown. `[network] max_mbps` sets your internet speed so the
network graph turns red when it's maxed out.

## 4. Discord (optional)

The Discord widget shows your call (who's talking, muted, on camera),
recent notifications (click one to open it), a server's voice channels,
and which friends are online. Discord only lets its local connection be
used by an application's owner and testers, so you make your own
application. It's free and takes a few minutes:

1. Go to <https://discord.com/developers/applications> and click
   **New Application**. Any name works.
2. On the **OAuth2** page, copy the **Client ID**, reset and copy the
   **Client Secret**, and under **Redirects** add `http://localhost`.
   Save.
3. Open settings and fill in `[discord]` with the id and secret, then
   add a widget with `kind = "discord"` to a layout.
4. Discord asks you to approve Wallflex the first time. After that it
   signs in by itself.

For who's online and the FRIENDS page, also add the application's bot:

1. On the **Bot** page, reset and copy the token into `bot_token`, and
   turn on **Presence Intent** and **Server Members Intent**.
2. On **OAuth2 > URL Generator**, tick `bot`, open the link it makes
   and add the bot to the servers your friends are in. It stays
   invisible and needs no permissions.
3. In Discord, turn on **Settings > Advanced > Developer Mode**, then
   right-click each friend > **Copy User ID** and add the ids to
   `friends`.

Keep the secret and the bot token to yourself: anyone with them can act
as your application. Wallflex only stores them in your settings file
and its sign-in token in `discord-token.json` beside it.

## 5. Uninstall

If you used the setup: **Settings > Apps > Installed apps > Wallflex >
Uninstall**. It closes Wallflex, stops it starting with Windows, and
asks whether to delete your settings too.

If you unzipped it:

1. Tray menu: untick **Start with Windows**, then **Quit Wallflex**.
2. Delete the folder you unzipped and `%APPDATA%\Wallflex`.

## Command line

`wallflex --help` lists everything. The useful ones:

```
wallflex --quit                stop the running copy
wallflex --config other.toml   run with another settings file
wallflex --sensors             print one sensor reading and exit
```
