#!/bin/sh
# TEMPORARY (PR diagnostics only — delete with the follow-up commit).
#
# This sandbox cannot fetch GitHub Actions logs, so RUSTC_WRAPPER (set in
# .cargo/config.toml) re-emits the first lines of each rustc/clippy diagnostic
# as `::error::` workflow commands, which are readable as check annotations.
out=$("$@" 2>&1)
code=$?
printf '%s\n' "$out" | head -c 12000 | sed -n '/^[^ ]/s/^\(.\{0,240\}\).*/::error::rustc | \1/p' | head -40
printf '%s\n' "$out" >&2
exit $code
