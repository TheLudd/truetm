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
| `Ctrl+B h`     | Focus window to the left                     |
| `Ctrl+B j`     | Focus window below                           |
| `Ctrl+B k`     | Focus window above                           |
| `Ctrl+B l`     | Focus window to the right                    |
| `Ctrl+B Enter` | Swap focused window with master              |
| `Ctrl+B H`     | Decrease master width                        |
| `Ctrl+B L`     | Increase master width                        |
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

### Copy Mode (Vim-style Scrollback)

Enter copy mode with `Ctrl+B s`. Supports numeric counts (e.g., `5j` to move 5 lines down).

#### Basic Movement

| Key           | Action                              |
| ------------- | ----------------------------------- |
| `h/j/k/l`     | Move cursor left/down/up/right      |
| `0`           | Move to start of line               |
| `$`           | Move to end of line                 |
| `^`           | Move to first non-blank character   |
| `g`           | Go to top of scrollback             |
| `G`           | Go to bottom (live view)            |
| `H/M/L`       | Move to top/middle/bottom of screen |
| `PgUp/PgDown` | Page up/down                        |

#### Word Motions

| Key   | Action                              |
| ----- | ----------------------------------- |
| `w/W` | Move to start of next word/WORD     |
| `b/B` | Move to start of previous word/WORD |
| `e/E` | Move to end of word/WORD            |

#### Search

| Key   | Action                         |
| ----- | ------------------------------ |
| `/`   | Search forward (regex)         |
| `?`   | Search backward (regex)        |
| `n`   | Jump to next match             |
| `N`   | Jump to previous match         |
| `f/F` | Find char forward/backward     |
| `t/T` | Find char (till) forward/back  |
| `;`   | Repeat last find               |
| `,`   | Repeat last find (reverse)     |

#### Visual Mode & Text Objects

| Key   | Action                              |
| ----- | ----------------------------------- |
| `v`   | Start character-wise visual select  |
| `V`   | Start line-wise visual select       |
| `y`   | Yank (copy) selection to clipboard  |
| `iw`  | Select inner word                   |
| `aw`  | Select around word (includes space) |
| `i"`  | Select inside quotes                |
| `a"`  | Select around quotes                |
| `i(`  | Select inside parentheses           |
| `a(`  | Select around parentheses           |
| `i[`  | Select inside brackets              |
| `i{`  | Select inside braces                |

#### Exit

| Key         | Action         |
| ----------- | -------------- |
| `q` / `Esc` | Exit copy mode |

Scrollback stores up to 10,000 lines of history per window.

### Mouse

| Action         | Effect                                      |
| -------------- | ------------------------------------------- |
| Click and drag | Select text within a pane                   |
| Scroll wheel   | Scroll through scrollback (enters copy mode)|

## Configuration

truetm follows the dwm philosophy: configuration is done at compile time by editing `src/config.rs`. This file contains all keybindings and settings in a readable format.

## Author

Fully vibe coded with [Claude Code](https://claude.com/claude-code) and Opus 4.5.

## License

[Unlicense](UNLICENSE) - Public domain. Do whatever you want.

