#!/usr/bin/env bash
# Start the vendored wptserve on ports 8000-8002, run "$@" against it, and
# tear the server process group down on exit. Shared verbatim by wpt.yml and
# coverage.yml, which differ only in the command they pass in.
set -euo pipefail
log="${RUNNER_TEMP:-${TMPDIR:-/tmp}}/wptserve.log"
(
  cd vendor/wpt
  exec setsid python3 -c 'from tools import localpaths; from tools.serve import serve; raise SystemExit(serve.main())' \
    --config ../../den-core/tests/wpt-serve.json --no-h2
) >"$log" 2>&1 &
pid=$!
cleanup() {
  kill -TERM -- -"$pid" 2>/dev/null || true
  wait "$pid" 2>/dev/null || true
}
trap cleanup EXIT INT TERM

ready=false
for _ in {1..60}; do
  if ! kill -0 "$pid" 2>/dev/null; then
    break
  fi
  if curl --fail --silent --output /dev/null http://127.0.0.1:8000/resources/testharness.js \
    && curl --fail --silent --output /dev/null http://127.0.0.1:8001/resources/testharness.js \
    && python3 -c 'import socket; socket.create_connection(("127.0.0.1", 8002), 1).close()'; then
    ready=true
    break
  fi
  sleep 1
done
if [[ "$ready" != true ]]; then
  tail -200 "$log"
  exit 1
fi

"$@"
