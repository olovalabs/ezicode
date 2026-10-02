#!/bin/sh
# TEMPORARY (PR diagnostics only — removed together with .cargo/config.toml).
#
# This sandbox cannot fetch Actions logs, so RUSTC_WRAPPER re-emits the first
# lines of failing compiler output as `::error::` workflow commands, which are
# readable as check annotations.
out=$("$@" 2>&1)
code=$?
if [ "$code" -ne 0 ]; then
  printf '%s\n' "$out" | sed '/^$/d' | head -14 | while IFS= read -r line; do
    printf '::error::r | %s\n' "$(printf '%s' "$line" | cut -c1-220 | sed 's/%/%25/g')"
  done
fi
printf '%s\n' "$out" >&2
exit $code
