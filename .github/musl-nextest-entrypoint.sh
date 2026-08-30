#!/bin/sh
set -eu
chown den:den /cargo /target
exec su-exec den cargo nextest "$@"
