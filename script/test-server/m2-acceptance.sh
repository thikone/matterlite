#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
#
# M2 acceptance (docs/M2.md step 7): the sync engine, headless (`mm-cli sync`), against the
# test server (docs/MatterLite-TestServer.md). Run from the repository root, on Windows in Git
# Bash or on Linux/macOS, with ssh access to the VM:
#
#   script/test-server/m2-acceptance.sh            # against the server as it is
#   RESET=1 script/test-server/m2-acceptance.sh    # reset and seed it first
#
# Scenarios: first start, restart from the cache, an offline gap the server can't replay,
# a post resent from the outbox after a restart, and an outbox post that never arrived.
# The checks compare with the server's own numbers, so they don't depend on the seeded state.
# The run adds posts to #dev (as bob and alice); RESET=1 cleans up before the next run.
#
# Needs: cargo, curl, jq, ssh (key login to the VM, passwordless sudo there for nft and ss).

set -euo pipefail
cd "$(dirname "$0")/../.."

VM=${VM:-andrew@ubuntu}
URL=${URL:-https://ubuntu}

if [ "${RESET:-}" = 1 ]; then
    echo "== reset and seed the test server"
    ssh "$VM" 'bash -s' < script/test-server/reset.sh > /dev/null
    ssh "$VM" 'bash -s' < script/test-server/seed.sh > /dev/null
fi

# jq on Windows ends lines with CRLF; the checks compare plain text.
jq() {
    command jq "$@" | tr -d '\r'
}

cargo build -q -p mm_cli
CLI=target/debug/mm-cli
eval "$(ssh "$VM" cat mm-test-server/credentials.env)"
API=$MM_SERVER_URL/api/v4
DATA=$(mktemp -d)
LOGS=$DATA/logs
mkdir -p "$LOGS"
CLIENT_IP=$(ssh "$VM" 'echo ${SSH_CLIENT%% *}')
echo "== data in $DATA, client $CLIENT_IP, server $URL"

passed=0
failed=0
ok() {
    echo "  ok    $*"
    passed=$((passed + 1))
}
bad() {
    echo "  FAIL  $*"
    failed=$((failed + 1))
}
# expect <log> <extended regex> <what>
expect() {
    if grep -qE -- "$2" "$LOGS/$1.log"; then
        ok "$3"
    else
        bad "$3"
        echo "        (pattern: $2; last lines of $1.log:)"
        tail -n 15 "$LOGS/$1.log" | sed 's/^/        | /'
    fi
}
# expect_equal <actual> <expected> <what>
expect_equal() {
    if [ "$1" = "$2" ]; then ok "$3"; else bad "$3 (got $1, expected $2)"; fi
}

as() {
    local token=$1
    shift
    curl -s -H "Authorization: Bearer $token" -H 'Content-Type: application/json' "$@"
}
ALICE=$(as "$MM_ALICE_TOKEN" "$API/users/me" | jq -r .id)
DEV=$(as "$MM_ALICE_TOKEN" "$API/teams/name/matterlite/channels/name/dev" | jq -r .id)

# Stops the client's traffic to the server's HTTPS port: `reject` (connection refused) or
# `drop` (requests hang), and kills its open connections.
block() {
    ssh "$VM" "sudo nft add table inet mlaccept && \
        sudo nft add chain inet mlaccept input '{ type filter hook input priority -10; }' && \
        sudo nft add rule inet mlaccept input ip saddr $CLIENT_IP tcp dport 443 $1 && \
        sudo ss -K state established '( sport = :443 )' dst $CLIENT_IP" > /dev/null 2>&1
}
unblock() {
    ssh "$VM" 'sudo nft delete table inet mlaccept' > /dev/null 2>&1 || true
}
trap unblock EXIT

# sync <name>: the engine with commands from stdin (this function's stdin), log in <name>.log.
sync() {
    MM_TOKEN=$MM_ALICE_TOKEN timeout 300 "$CLI" "$URL" sync --data "$DATA" > "$LOGS/$1.log" 2>&1 || true
}
# Posts in #dev with exactly this message, on the server.
count_on_server() {
    as "$MM_ALICE_TOKEN" "$API/channels/$DEV/posts?per_page=200" |
        jq --arg text "$1" '[.posts[] | select(.message == $text and .delete_at == 0)] | length'
}

echo "== 1. first start, empty cache"
{
    sleep 12
    echo unread
    echo "show history"
    echo quit
} | sync first
expect first 'ready \(bootstrapped\)' "bootstraps"
expect first 'Sync\(SyncStatus \{ queued: 0, running: 0, done: [0-9]+, backfilling: 0' \
    "the eager sync goes idle with every channel backfilled"
expect first '~History \(6[0-9]{2} items' "#history is fully loaded (600+ posts)"
read -r UNREAD MENTIONS < <(as "$MM_ALICE_TOKEN" \
    "$API/users/me/teams/unread?include_collapsed_threads=true" |
    jq -r --arg team "$(as "$MM_ALICE_TOKEN" "$API/teams/name/matterlite" | jq -r .id)" \
        '.[] | select(.team_id == $team) | "\(.msg_count_root) \(.mention_count_root)"')
expect first "\[MatterLite\] $UNREAD unread, $MENTIONS mentions" \
    "team badge equals the server ($UNREAD unread, $MENTIONS mentions)"

echo "== 2. restart from the cache"
{
    sleep 4
    echo quit
} | sync restart
expect restart 'hydrated from the cache: [0-9]+ channels, [0-9]+ posts' "hydrates before any request"
expect restart 'ready \(resumed the saved session' "resumes the saved WebSocket session"

echo "== 3. offline gap the server can't replay (300 events)"
{
    sleep 4
    block reject
    for i in $(seq 300); do
        as "$MM_BOB_TOKEN" -X POST "$API/posts" \
            -d "{\"channel_id\":\"$DEV\",\"message\":\"gap burst $i\"}" > /dev/null
    done
    unblock
    sleep 20
    echo "search gap burst 300"
    echo quit
} | sync gap
expect gap 'Connection\(Offline' "notices the outage"
expect gap 'Resynced' "resyncs after the server lost the session"
expect gap '~Dev: gap burst 300' "the last post of the gap is in the local cache"

echo "== 4. outbox: stopped while sending, restarted within 30 s"
RESEND="outbox resend $(date +%s)"
{
    sleep 4
    block drop
    echo "send dev $RESEND"
    sleep 3
    echo quit
} | sync outbox-stop
unblock
expect outbox-stop 'changed: 1 pending' "the post is pending when the engine stops"
{
    sleep 8
    echo quit
} | sync outbox-resend
sleep 1
expect_equal "$(count_on_server "$RESEND")" 1 "re-sent once: exactly one copy on the server"

echo "== 5. outbox: stopped while sending, restarted after 30 s"
LOST="outbox lost $(date +%s)"
{
    sleep 4
    block drop
    echo "send dev $LOST"
    sleep 3
    echo quit
} | sync outbox-stop-2
unblock
echo "   (waiting 32 s, past the server's 30 s dedupe window)"
sleep 32
{
    sleep 8
    echo "show dev"
    echo quit
} | sync outbox-reconcile
expect outbox-reconcile "$LOST \(failed\)" "shown as failed after the lookup"
expect_equal "$(count_on_server "$LOST")" 0 "not re-sent blindly: no copy on the server"

echo "== $passed passed, $failed failed (logs in $LOGS)"
[ "$failed" -eq 0 ]
