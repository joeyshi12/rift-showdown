#!/usr/bin/env bash
# Background server for the E2E tests: ./dev-server.sh start|stop [port]
# Logs go to target/dev-server/server.log.
cd "$(dirname "$0")"
DIR=target/dev-server
PIDF=$DIR/server.pid
mkdir -p "$DIR"
case "${1:-start}" in
  stop) [ -f "$PIDF" ] && kill "$(cat "$PIDF")" 2>/dev/null; rm -f "$PIDF" ;;
  start)
    [ -f "$PIDF" ] && kill "$(cat "$PIDF")" 2>/dev/null
    RIFT_BIND="127.0.0.1:${2:-8787}" nohup ./target/release/rift-server > "$DIR/server.log" 2>&1 &
    echo $! > "$PIDF"; sleep 0.7; curl -sf "http://127.0.0.1:${2:-8787}/healthz" >/dev/null && echo "server up on ${2:-8787}" ;;
esac
