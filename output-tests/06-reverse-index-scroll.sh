#!/usr/bin/env bash
# Bug #6: ESC M (Reverse Index) at the top of the scroll region only moved
# the cursor instead of scrolling the region down. Pagers (less, delta)
# scroll upward with "home + RI", so pressing k in a pager repainted only
# the top row while the rest of the screen stayed frozen.
#
# Correct: lines read 1..12 top to bottom, then after the scroll the screen
#          shows "NEW" at the top followed by 1..11 (12 pushed off the bottom).
# Buggy:   "NEW" overwrites line 1, lines 2..12 stay where they were.

clear
for i in $(seq 1 12); do printf 'line %d\n' "$i"; done
sleep 1
printf '\x1b[H\x1bMNEW\n'
printf '\x1b[14;1H'
echo 'expected: NEW, line 1, line 2, ... line 11 (line 12 gone)'
