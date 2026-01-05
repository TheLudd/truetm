# simplex

A terminal multiplexer inspired by [dvtm](https://www.brain-dump.org/projects/dvtm/) with truecolor support.

## Features

- **Truecolor support** - Full 24-bit RGB color passthrough
- **dvtm-style tagging** - Windows can have multiple tags, views can show multiple tags
- **Tiling layout** - Master window on left, stack on right

## Installation

### From source

```sh
git clone https://github.com/yourusername/simplex
cd simplex
make
sudo make install
```

Or manually:

```sh
cargo build --release
sudo cp target/release/simplex /usr/local/bin/
sudo cp simplex.1 /usr/local/share/man/man1/  # optional: install man page
```

## Usage

```sh
simplex
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
| `Ctrl+B 1-9`   | Focus window by number                       |
| `Ctrl+B a`     | Toggle broadcast mode (input to all windows) |
| `Ctrl+B q`     | Quit simplex                                 |
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

## Building

Requires Rust 1.70+

```sh
cargo build --release
```

## Configuration

simplex follows the dwm philosophy: configuration is done at compile time by editing the source code. Key settings can be found in `src/main.rs`.

## License

MIT
