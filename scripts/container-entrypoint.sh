#!/bin/sh
set -eu
if [ "${CADENCE_PROVIDER:-librespot}" != soloist ]; then
    exec /usr/local/bin/cadence
fi
umask 077
export HOME=/tmp/home XDG_CONFIG_HOME=/tmp/config XDG_STATE_HOME=/tmp/state
export XDG_RUNTIME_DIR=/tmp/runtime-cadence
mkdir -p "$XDG_RUNTIME_DIR" /soloist-data
# Mount the official binary privately; do not redistribute it in the image.
cp /opt/soloist/soloist /soloist-data/soloist
chmod 700 /soloist-data/soloist
pulseaudio --daemonize=yes --exit-idle-time=-1 --load='module-null-sink sink_name=cadence_test'
/soloist-data/soloist --device-name "${SPOTIFY_DEVICE_NAME:-Cadence}" \
    --api-key "$(cat /run/secrets/soloist_key)" --data-dir /soloist-data \
    --cache-dir /tmp/soloist-cache --cache-size 100 --ws 127.0.0.1:9090 &
soloist_pid=$!
bot_pid=
cleanup() {
    trap '' TERM INT
    [ -z "$bot_pid" ] || kill "$bot_pid" 2>/dev/null || true
    kill "$soloist_pid" 2>/dev/null || true
    [ -z "$bot_pid" ] || wait "$bot_pid" 2>/dev/null || true
    wait "$soloist_pid" 2>/dev/null || true
    pulseaudio --kill 2>/dev/null || true
}
trap cleanup EXIT
trap 'exit 0' TERM INT
/usr/local/bin/cadence &
bot_pid=$!
set +e
wait "$bot_pid"
status=$?
exit "$status"
