#!/usr/bin/env bash
# Bug #3b: OSC parse buffer bails to Normal after 256 bytes, spilling the
# rest as literal text. This is an OSC 8 hyperlink with a ~320-char URL,
# like the file:// links Claude Code emits.
#
# Correct: just "CLICK" (possibly rendered as a hyperlink)
# Buggy:   a stream of literal zeros printed before "CLICK"

printf '\x1b]8;;http://example.com/%0300d\x1b\\CLICK\x1b]8;;\x1b\\\n' 0
