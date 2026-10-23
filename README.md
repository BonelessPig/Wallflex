# Wallflex

Lightweight desktop widgets for Windows, drawn behind the desktop icons.
No Wallpaper Engine, no browser engine, no admin rights.

**Status: milestone 1.** Wallflex paints a solid background on every
monitor, between the wallpaper and the icons. Widgets come next.

## Build

Needs Rust (stable, 1.88 or later) with the MSVC toolchain.

```
cargo build --release
```

The exe is `target\release\wallflex.exe`.

## Run

```
wallflex                      black background on every monitor
wallflex --color 203040       any colour, as RRGGBB
wallflex --render d2d-hw      paint with Direct2D instead of GDI
wallflex --quit               stop the running copy
wallflex --diagnose           show how Explorer's desktop windows are laid out
```

Release builds have no console window; stop them with `--quit`. Debug
builds (`cargo run`) keep a console, and Ctrl+C stops them.

## Measuring

`tools\measure.ps1` starts each render mode in turn and reports RAM, CPU,
threads and whether a GPU driver was loaded:

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
