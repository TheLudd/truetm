#!/usr/bin/env bash
# Bug #1 (buffer side): wide characters counted as 1 column in ScreenBuffer.
# Prints an emoji, then asks the terminal for the cursor position (DSR 6n).
#
# Correct: col: 3  (emoji occupies 2 columns)
# Buggy:   col: 2  (truetm counted it as 1 column)

printf '🙂\x1b[6n'
IFS= read -rsd R p
printf '\ncol: %s\n' "${p##*;}"
