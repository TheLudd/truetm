# truetm

A terminal multiplexer inspired by [dvtm](https://www.brain-dump.org/projects/dvtm/) with truecolor support.

## Features

- **Truecolor support** - Full 24-bit RGB color passthrough
- **dvtm-style tagging** - Windows can have multiple tags, views can show multiple tags
- **Tiling layout** - Master window on left, stack on right

## Installation

### Requirements

- Rust 1.70+ and Cargo
- A C compiler (gcc or clang)

On Debian/Ubuntu:
```sh
apt install build-essential
```

On Arch:
```sh
pacman -S base-devel
```

On macOS, install Xcode Command Line Tools:
```sh
xcode-select --install
```

### From source

```sh
git clone https://github.com/theludd/truetm
cd truetm
cargo build --release
sudo cp target/release/truetm /usr/local/bin/
```

## Usage

```sh
truetm
```

## Keybindings

All keybindings use `Ctrl+B` as the prefix key.

### Window Management

| Key            | Action                                       |
| -------------- | -------------------------------------------- |
| `Ctrl+B c`     | Create new window                            |
| `Ctrl+B x`     | Close focused window                         |
| `Ctrl+B j`     | Focus next window                            |
| `Ctrl+B k`     | Focus previous window                        |
| `Ctrl+B Enter` | Swap focused window with master              |
| `Ctrl+B h`     | Decrease master width                        |
| `Ctrl+B l`     | Increase master width                        |
| `Ctrl+B z`     | Toggle zoom (fullscreen focused window)      |
| `Ctrl+B 1-9`   | Focus window by number                       |
| `Ctrl+B a`     | Toggle broadcast mode (input to all windows) |
| `Ctrl+B q`     | Quit truetm                                  |
| `Ctrl+B b`     | Send literal Ctrl+B to window                |

### Tags (Workspaces)

| Key          | Action                                            |
| ------------ | ------------------------------------------------- |
| `Ctrl+B v N` | View tag N (1-9)                                  |
| `Ctrl+B t N` | Set tag N on focused window (replaces other tags) |
| `Ctrl+B T N` | Toggle tag N on focused window                    |

Tags work like virtual desktops but more flexible:
- A window can have multiple tags (appear in multiple views)
- Closing the last window in a tag returns to the previously visited tag

### Scrollback

| Key           | Action                         |
| ------------- | ------------------------------ |
| `Ctrl+B [`    | Enter scroll mode              |
| `k` / `Up`    | Scroll up one line             |
| `j` / `Down`  | Scroll down one line           |
| `PgUp`        | Scroll up half page            |
| `PgDown`      | Scroll down half page          |
| `g`           | Go to top of scrollback        |
| `G`           | Go to bottom (live view)       |
| `q` / `Esc`   | Exit scroll mode               |

Scrollback stores up to 1000 lines of history per window.

## Configuration

truetm follows the dwm philosophy: configuration is done at compile time by editing `src/config.rs`. This file contains all keybindings and settings in a readable format.

## Author

Fully vibe coded with [Claude Code](https://claude.com/claude-code) and Opus 4.5.

## License

[Unlicense](UNLICENSE) - Public domain. Do whatever you want.

