# Terminal emulation tests

Three tiers, from fully automated to eyeball-only:

## 1. Fully automated (run these)

```sh
cargo test              # unit tests: parser/grid behavior in src/render.rs
cargo test --test e2e   # end-to-end: boots the real truetm binary in a PTY,
                        # types into the pane shell, asserts on the screen
                        # truetm draws, and runs checks.sh inside the pane
```

No terminal or human required - suitable for CI.

## 2. Self-verifying, but needs a live terminal: `checks.sh`

```sh
./output-tests/checks.sh
```

Run it *inside* the terminal you want to test (a truetm pane, st, xterm,
foot, ...). Each check prints a scenario, asks the terminal where the
cursor ended up (cursor position report, `CSI 6n`), and prints PASS or
FAIL. Exit code 0 means everything passed.

Because it tests whatever terminal it runs in, it doubles as a conformance
probe: run it in st or xterm to see how a reference terminal behaves.

The e2e test runs this same script inside truetm automatically, so tier 2
is only needed when testing a terminal by hand.

## 3. Visual checks: `01`-`05`

The numbered scripts reproduce the original bugs in a form you can see.
Each documents what correct and buggy output look like in its header.
They predate `checks.sh`, which covers the same ground with PASS/FAIL
output - keep using them when you want to *watch* a bug happen, e.g. `05`
which needs a manual tag-switch to force a full repaint.

## What is covered

- Wide characters (emoji, CJK): cell width, cursor position, wrapping,
  compositor alignment
- Zero-width characters: combining accents, VS16 emoji presentation
- Kitty keyboard protocol sequences (`CSI ?u`, `CSI >1u`, `CSI <u`) not
  being misread as cursor commands; same for XTRESTORE/XTMODKEYS
- Oversized CSI/OSC sequences (long truecolor SGR chains, long OSC 8
  hyperlinks, long titles) being discarded instead of spilled as text
- Multi-byte/unicode window titles not crashing the header renderer
- Deferred wrap: cursor reported at the last column, next char wraps
