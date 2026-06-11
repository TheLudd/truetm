#!/usr/bin/env bash
# Bug #3a: CSI parse buffer bails to Normal after 64 bytes, spilling the
# rest of the sequence as literal text. This is one legal SGR sequence
# with ~69 bytes of parameters (truecolor fg+bg twice plus attributes).
#
# Correct: just a styled "HI"
# Buggy:   literal junk like ";60mHI" printed before/instead of styled HI

printf '\x1b[38;2;200;100;50;48;2;30;30;30;1;3;4;9;38;2;100;200;50;48;2;60;60;60mHI\x1b[0m\n'
