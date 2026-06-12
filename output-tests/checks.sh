#!/usr/bin/env bash
# Self-verifying terminal emulation checks.
#
# Run this INSIDE the terminal under test (a truetm pane, st, xterm, ...).
# Each check prints a scenario, then asks the terminal where the cursor
# ended up (DSR, CSI 6n) and compares against what a conformant terminal
# must answer. No human judgment needed: every line is PASS or FAIL and
# the exit code is 0 only if everything passed.
#
# This also works as a conformance probe for OTHER terminals: running it
# in st/xterm/foot shows how a reference terminal behaves.

pass=0
fail=0

# Ask the terminal for the cursor column (1-based). Empty on timeout.
# Talks to /dev/tty directly: this runs inside $(...) where stdout is captured.
cursor_col() {
    local p=
    printf '\x1b[6n' > /dev/tty
    IFS= read -rsd R -t 2 p < /dev/tty
    printf '%s' "${p##*;}"
}

# check <name> <expected-col> <bytes...>
# Prints the bytes at column 1 of the current line, reads the cursor
# column back, erases the scenario, and reports.
check() {
    local name=$1 expected=$2 actual
    shift 2
    printf '\r\x1b[K'
    printf '%b' "$@"
    actual=$(cursor_col)
    printf '\r\x1b[K'
    if [[ $actual == "$expected" ]]; then
        printf 'PASS %s\n' "$name"
        ((pass++))
    else
        printf 'FAIL %s (expected col %s, got %s)\n' "$name" "$expected" "${actual:-no-response}"
        ((fail++))
    fi
}

# --- basic sanity ----------------------------------------------------------
check "ascii width"                4 'abc'

# --- unicode width ---------------------------------------------------------
check "emoji is double width"      3 '🙂'
check "CJK is double width"        5 '日本'
check "VS16 emoji presentation"    3 $'⚠️'   # ⚠️ = narrow char + VS16
check "combining accent is zero width" 2 $'é'

# --- sequences that must not move the cursor -------------------------------
# Each saves nothing: cursor must stay right after the printed text.
check "kitty keyboard query (CSI ?u)"     3 '\x1b7ab\x1b[?u'
check "kitty keyboard push/pop (>1u <u)"  3 '\x1b7ab\x1b[>1u\x1b[<u'
check "XTRESTORE private modes (?1004r)"  3 '\x1b7ab\x1b[?1004r'
check "XTMODKEYS (CSI >4;2m)"             3 '\x1b7ab\x1b[>4;2m'

# --- long sequences must be consumed, never spilled as text ----------------
check "long truecolor SGR (~70 bytes)" 3 \
    '\x1b[38;2;200;100;50;48;2;30;30;30;1;3;4;9;38;2;100;200;50;48;2;60;60;60mHI\x1b[0m'

huge_csi='\x1b['
for _ in $(seq 150); do huge_csi+='1;'; done
huge_csi+='m'
check "oversized CSI (~300 bytes) discarded" 3 "${huge_csi}HI\\x1b[0m"

big=$(printf 'a%.0s' $(seq 4200))
check "oversized OSC title (BEL) discarded" 2 "\\x1b]0;${big}\\x07X"
check "oversized OSC 8 link (ST) discarded" 6 "\\x1b]8;;http://x/${big}\\x1b\\\\CLICK\\x1b]8;;\\x1b\\\\"

# --- titles with multi-byte characters must not crash the terminal ---------
printf '\x1b]0;⚠️🙂 very long unicode title %s\x07' "$big"
check "unicode title set (no crash)" 3 'ok'
printf '\x1b]0;checks done\x07'

# --- wrap behavior ----------------------------------------------------------
cols=$(tput cols 2>/dev/null || echo 80)
full_line=$(printf 'x%.0s' $(seq "$cols"))
check "DSR at right margin (wrap pending)" "$cols" "$full_line"

# --- summary ----------------------------------------------------------------
echo
if (( fail == 0 )); then
    printf 'ALL CHECKS PASSED (%d)\n' "$pass"
    exit 0
else
    printf 'CHECK FAILURES: %d of %d\n' "$fail" $((pass + fail))
    exit 1
fi
