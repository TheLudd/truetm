#!/usr/bin/env bash
# Bug #2: CSI ?u (kitty keyboard query) misparsed as ANSI restore-cursor.
# CONFIRMED on 2026-06-11 — kept as a regression test.
#
# Correct: "hello world" together at row 5, column 5.
# Buggy:   "hello" at row 5, " world" back at the saved cursor position
#          (the line below this prompt).

printf '\x1b7\x1b[5;5Hhello\x1b[?u world\n'
