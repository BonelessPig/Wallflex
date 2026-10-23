# Wallflex

Lightweight desktop widgets for Windows, drawn behind the desktop icons.
No Wallpaper Engine, no browser engine, no admin rights.

**Status: milestone 3.** Wallflex paints a solid background on every
monitor, between the wallpaper and the icons, with three widgets in the
bottom-right corner, updated once a second:

- **CPU threads:** a column per thread, paired by core, with the clock.
- **Network:** the last minute of download and upload.
- **GPU rings:** per GPU, load, VRAM, temperature, power and fan.

plus RAM use and SSD temperatures. Sizes follow the monitor's height, so
the widgets look the same on any resolution.

## Build

Needs Rust (stable, 1.88 or later) with the MSVC toolchain.

```
cargo build --release
```

The exe is `target\release\wallflex.exe`.

## Run

```
wallflex                      black background and sensor panel
wallflex --color 203040       any colour, as RRGGBB
wallflex --sensors            print one sensor reading and exit
wallflex --preview 2160 a.bmp save the widgets as they'd look on a 2160p monitor
wallflex --quit               stop the running copy
wallflex --diagnose           show how Explorer's desktop windows are laid out
```

Release builds have no console window; stop them with `--quit`. Debug
builds (`cargo run`) keep a console, and Ctrl+C stops them.

## Measuring

`tools\measure.ps1` runs the release build and reports RAM, CPU, threads
and whether a GPU driver was loaded:

```
powershell -ExecutionPolicy Bypass -File tools\measure.ps1 -Seconds 30
```

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

Unless you explicitly state otherwise, any contribution intentionally
submitted for inclusion in the work by you, as defined in the Apache-2.0
license, shall be dual licensed as above, without any additional terms or
conditions.
