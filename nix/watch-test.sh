# checks.<system>.watch: watch's alerting rules, against a console and a store
# served from files (curl reads file:// URLs), with no notify URL, so each
# notice is logged as "would notify". Run by `nix flake check`; WATCH is the
# watch program.
set -euo pipefail
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/console/api" "$work/store"
export WATCH_STATE="$work/state" WATCH_HOST=testhost WATCH_CONSOLE="file://$work/console"
unset WATCH_NOTIFY WATCH_NOTIFY_FILE WATCH_HEARTBEAT WATCH_HEARTBEAT_FILE

console() { # SERVICE STATE
  printf '{"services":{"%s":{"health":{"http_status":200,"body":{"state":"%s","detail":"d"}}}}}' "$1" "$2" > "$work/console/api/state"
}
run() { "$WATCH" --once 2> "$work/log" || true; cat "$work/log" >&2; }
says() { grep -qF -- "$1" "$work/log" || { echo "FAIL: expected: $1" >&2; exit 1; }; }
silent() { if grep -q "would notify" "$work/log"; then echo "FAIL: expected no notice ($1)" >&2; exit 1; fi; }

echo "== a failed service: told after two runs, once, then resolved"
console relay failed
run; silent "first sighting"
run; says "would notify: testhost: relay: failed (d)"
run; silent "not repeated before WATCH_REPEAT_HIGH"
WATCH_REPEAT_HIGH=0 run; says "(still)"
console relay ready
run; says "would notify: testhost: resolved (1)"
run; silent "resolved once"

echo "== a blip of one run says nothing"
console relay failed; run; console relay ready; run; silent "blip"

echo "== idle is fine; degraded only after a while"
console relay idle; run; run; silent "idle"
console relay degraded; run; run; silent "degraded under WATCH_DEGRADED_AFTER"
export WATCH_DEGRADED_AFTER=0
run; run; says "relay: degraded for 0 min"
unset WATCH_DEGRADED_AFTER
console relay ready; run; says "resolved"

echo "== the console not answering"
rm "$work/console/api/state"; run; run; says "console not answering"
console relay ready; run; says "resolved"

echo "== the store's /health"
printf '{"state":"failed","detail":"no volumes"}' > "$work/store/health"
WATCH_STORE="file://$work/store" run; WATCH_STORE="file://$work/store" run; says "store: failed (no volumes)"
run; says "resolved"

echo "== disks: a missing mount is high priority; a roomy one is fine"
WATCH_DISKS="/ $work/not-there" run; WATCH_DISKS="/ $work/not-there" run
says "not-there: missing"
WATCH_DISK_MIN_FREE_GIB=0 WATCH_DISKS="$work" run; WATCH_DISK_MIN_FREE_GIB=0 WATCH_DISKS="$work" run
says "not mounted (it is part of"

echo "== containers expected without a Docker socket is a problem"
WATCH_SERVICES="a,b" run; says "2 containers expected but no WATCH_DOCKER_SOCKET"

echo "== an order gateway's port is refused"
if WATCH_CONSOLE=http://127.0.0.1:7710 "$WATCH" --once 2> "$work/log"; then echo "FAIL: ran against 7710" >&2; exit 1; fi
says "refusing http://127.0.0.1:7710"
if WATCH_NEVER_PORTS="7700,7701" WATCH_STORE=http://127.0.0.1:7701 "$WATCH" --once 2> "$work/log"; then echo "FAIL: ran against 7701" >&2; exit 1; fi
says "port 7701"
echo "watch: all checks passed"
