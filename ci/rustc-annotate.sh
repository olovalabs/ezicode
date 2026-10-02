#!/bin/sh
# TEMPORARY (PR diagnostics only — removed together with .cargo/config.toml).
#
# This sandbox cannot fetch Actions logs, so RUSTC_WRAPPER re-emits the first
# lines of failing compiler output as workflow commands on stderr, which cargo
# forwards to the step log, where the runner turns them into annotations.
if [ ! -f /tmp/rustc-annotate-ran ]; then
  : > /tmp/rustc-annotate-ran
  echo "::notice::wrapper active (cwd=$(pwd))" >&2
fi
out=$("$@" 2>&1)
code=$?
if [ "$code" -ne 0 ]; then
  printf '%s\n' "$out" | sed '/^$/d' | head -14 | while IFS= read -r line; do
    printf '::error::r | %s\n' "$(printf '%s' "$line" | cut -c1-200 | sed 's/%/%25/g')" >&2
  done
fi
printf '%s\n' "$out" >&2
exit $code
