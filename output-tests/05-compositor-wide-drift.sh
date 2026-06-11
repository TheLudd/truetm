#!/usr/bin/env bash
# Bug #1 (compositor side): the diff renderer assumes every glyph it writes
# advances the host cursor by 1 column, so a width-2 glyph makes everything
# after it in the same run drift one column right.
#
# After running this, force a full repaint (e.g. switch to another tag and
# back) and watch the line below:
#
# Correct: "🙂A" stays put, nothing left behind
# Buggy:   the "A" shifts, duplicates, or a stale half-glyph remains

printf '🙂A\n'
echo 'now switch tags away and back, then inspect the line above'
