#!/bin/sh
# Starts the server as uid 10001.
#
# Started as root (the `rootfix` build target, used on hosts that mount
# volumes owned by root), it gives the data directory to uid 10001 and drops
# all privileges before running the server. Started as uid 10001 (the default
# target), it runs the server directly.
set -eu

if [ "$(id -u)" = "0" ]; then
    data="${DC_DATA_DIR:-/data}"
    mkdir -p "$data"
    chown -R 10001:10001 "$data"
    chmod 0700 "$data"
    exec setpriv --reuid=10001 --regid=10001 --clear-groups \
        --inh-caps=-all --bounding-set=-all --no-new-privs -- "$@"
fi

exec "$@"
