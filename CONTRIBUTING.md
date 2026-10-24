# Contributing to Wallflex

Thanks for helping. Bug reports, ideas and pull requests are all
welcome. Everyone taking part is expected to follow the
[code of conduct](CODE_OF_CONDUCT.md).

## Reporting bugs and asking for features

Open an [issue](https://github.com/BonelessPig/Wallflex/issues/new/choose)
using one of the templates. For a bug, the most useful things are the
Wallflex version, your Windows version, your monitors, and the end of
`%APPDATA%\Wallflex\wallflex.log`. Remove any Discord ids, tokens or
secrets from the config before you paste it.

Security problems go through the [security policy](SECURITY.md), not
public issues.

## Before you start

For anything bigger than a small fix, open an issue first so we can
agree on the approach. Wallflex has one rule above the others:
**lightweight comes first**. Its budget is under 15 MB of RAM and under
0.5% CPU, with no GPU use between repaints, so a feature that costs
much more than that probably won't be merged, however nice it looks.

## Building and checking

You need Rust (stable, 1.88 or later) with the MSVC toolchain, on
Windows 10 or 11. Before opening a pull request, run:

```
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

CI runs the same checks, and a warning fails the build.

To see your change without touching your desktop, render the widgets
to a picture at a few monitor heights:

```
cargo run -- --preview 1080 a.bmp
cargo run -- --preview 2160 b.bmp
cargo run -- --preview 1440 c.bmp --widget discord
```

Use `--config my.toml` to try layouts without editing your own
settings. If your change could affect memory or CPU, measure it with
`tools\measure.ps1` before and after, and put the numbers in the pull
request.

## Code style

- `cargo fmt`, with lines up to 80 characters (never over 100).
- Clippy runs with `pedantic` and extra safety lints: no `unwrap`,
  `expect` or `panic!`, no unchecked indexing, arithmetic overflow or
  `as` casts. Tests may unwrap, expect, panic and index.
- `unsafe` is only allowed under `src/win32/`, and every `unsafe` block
  needs a `// SAFETY:` comment saying why it's sound.
- Every item, private ones too, has a doc comment.
- Check every fallible result. Ignore one only with `let _ =` and a
  comment saying why that's fine.
- No magic numbers: use named constants.
- Keep logic that doesn't need Windows in safe, unit-tested modules
  outside `src/win32/`.

## Pull requests

- Keep each pull request to one change, with a clear title.
- Add tests for new logic.
- Note user-visible changes under `## [Unreleased]` in
  [CHANGELOG.md](CHANGELOG.md).
- If you change settings, the menu or the installer, update
  `config.default.toml` and [docs/SETUP.md](docs/SETUP.md) to match.
- Screenshots must use sample data (`--preview`), never a capture of a
  real desktop with someone's Discord friends, servers or media.

## Licensing

Wallflex is licensed under MIT OR Apache-2.0. Unless you say otherwise,
anything you submit for inclusion is dual licensed the same way, with
no additional terms.

Only submit code you wrote or that's under a compatible licence. In
particular, don't copy code or assets from Wallpaper Engine Workshop
items or other projects whose licence doesn't allow it.
