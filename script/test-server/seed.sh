#!/usr/bin/env bash
# Seeds a fresh Mattermost server with deterministic MatterLite test data: users, teams,
# channels, DMs/GMs, posts covering the rendering rules, threads, reactions, files, a long
# history for pagination, unread/mention states, an MFA user, a bot and access tokens.
#
#   ssh andrew@ubuntu 'bash -s' < script/test-server/seed.sh
#
# Runs on the server host (uses `mmctl --local`). Exits early if the data is already there;
# reset.sh wipes the server and seeds again. Tokens and the MFA secret are written to
# ~/mm-test-server/credentials.env on the server host.
set -euo pipefail

API=http://localhost:8065/api/v4
PASSWORD="${PASSWORD:-matterlite}"
ADMIN_PASSWORD="${ADMIN_PASSWORD:-Sys@dmin-sample1}"
OUT_DIR="$HOME/mm-test-server"
CREDS="$OUT_DIR/credentials.env"
mkdir -p "$OUT_DIR"

step() { printf '==> %s\n' "$*"; }

# api METHOD PATH TOKEN [JSON] -> response body; fails on HTTP errors with the body shown.
api() {
  local out
  if ! out=$(curl -sS --fail-with-body -X "$1" -H "Authorization: Bearer $3" \
      -H 'Content-Type: application/json' ${4:+--data-binary "$4"} "$API$2"); then
    echo "API $1 $2 failed: $out" >&2
    return 1
  fi
  printf '%s' "$out"
}

login() {
  curl -sS --fail-with-body -o /dev/null -D - -H 'Content-Type: application/json' \
    --data-binary "$(jq -nc --arg l "$1" --arg p "$2" '{login_id:$l,password:$p}')" \
    "$API/users/login" | tr -d '\r' | awk -F': ' 'tolower($1)=="token"{print $2}'
}

if ! curl -fsS "$API/system/ping" >/dev/null; then
  echo "Server is not answering on $API" >&2
  exit 1
fi

if mmctl --local user search alice >/dev/null 2>&1; then
  echo "Already seeded (user alice exists). Run reset.sh for a fresh server."
  [[ -f "$CREDS" ]] && echo "Credentials: $CREDS"
  exit 0
fi

# ---------------------------------------------------------------------------------------------
step "System admin"
mmctl --local user create --username sysadmin --email sysadmin@example.com \
  --password "$ADMIN_PASSWORD" --system-admin --email-verified >/dev/null
ADMIN=$(login sysadmin "$ADMIN_PASSWORD")
# Skip the first-admin setup wizard in the web UI.
api POST /system/onboarding/complete "$ADMIN" '{"organization":"MatterLite","install_plugins":[]}' >/dev/null

# ---------------------------------------------------------------------------------------------
step "Users"
declare -A UID_OF TOKEN_OF
UID_OF[sysadmin]=$(api GET /users/username/sysadmin "$ADMIN" | jq -r .id)
TOKEN_OF[sysadmin]=$ADMIN

mkuser() { # username first last [nickname]
  local body
  body=$(jq -nc --arg u "$1" --arg f "$2" --arg l "$3" --arg n "${4:-}" --arg p "$PASSWORD" \
    '{username:$u,email:($u+"@example.com"),password:$p,first_name:$f,last_name:$l,nickname:$n}')
  UID_OF[$1]=$(api POST /users "$ADMIN" "$body" | jq -r .id)
}
mkuser alice Alice Anderson "Ali"
mkuser bob Bob Brown
mkuser carol Carol Chen
mkuser dave Dave Davis
mkuser erin Erin Evans
mkuser frank Frank Fischer       # MFA enabled below
mkuser outsider Olga Outsider    # on the team, not in #collab or #dev
mkuser loner Lars Loner          # in no team at all
mkuser deactivated Dee Activated # deactivated at the end
CROWD=()
for i in $(seq -w 1 11); do
  mkuser "crowd-$i" Crowd "Member $i"
  CROWD+=("crowd-$i")
done

for u in alice bob carol dave erin frank outsider; do
  TOKEN_OF[$u]=$(login "$u" "$PASSWORD")
done

ids() { local u out=(); for u in "$@"; do out+=("${UID_OF[$u]}"); done; printf '%s\n' "${out[@]}" | jq -R . | jq -sc .; }

# ---------------------------------------------------------------------------------------------
step "Teams"
mkteam() { # name display type
  api POST /teams "$ADMIN" "$(jq -nc --arg n "$1" --arg d "$2" --arg t "$3" '{name:$n,display_name:$d,type:$t}')" | jq -r .id
}
add_team_members() { # team_id users...
  local t=$1; shift
  local body
  body=$(ids "$@" | jq -c --arg t "$t" 'map({team_id:$t,user_id:.})')
  api POST "/teams/$t/members/batch" "$ADMIN" "$body" >/dev/null
}
T_MAIN=$(mkteam matterlite "MatterLite" O)
T_SECOND=$(mkteam second "Second Team" I)
add_team_members "$T_MAIN" alice bob carol dave erin frank outsider deactivated "${CROWD[@]}"
add_team_members "$T_SECOND" alice bob

# ---------------------------------------------------------------------------------------------
step "Channels"
declare -A CH
mkchannel() { # key team name display type purpose header members...
  local key=$1 team=$2 name=$3 display=$4 type=$5 purpose=$6 header=$7
  shift 7
  CH[$key]=$(api POST /channels "$ADMIN" "$(jq -nc --arg t "$team" --arg n "$name" --arg d "$display" \
    --arg ty "$type" --arg p "$purpose" --arg h "$header" \
    '{team_id:$t,name:$n,display_name:$d,type:$ty,purpose:$p,header:$h}')" | jq -r .id)
  [[ $# -gt 0 ]] && api POST "/channels/${CH[$key]}/members" "$ADMIN" "{\"user_ids\":$(ids "$@")}" >/dev/null
  return 0
}
CH[town]=$(api GET "/teams/$T_MAIN/channels/name/town-square" "$ADMIN" | jq -r .id)
CH[offtopic]=$(api GET "/teams/$T_MAIN/channels/name/off-topic" "$ADMIN" | jq -r .id)
mkchannel dev "$T_MAIN" dev "Dev" O "Markdown, threads, reactions, files" \
  "Rendering showcase · see **pinned** posts · [Mattermost](https://mattermost.com)" \
  sysadmin alice bob carol dave erin frank
mkchannel collab "$T_MAIN" collab "Collab" O "Shared drafts (matterlite-collab plugin)" "" \
  alice bob carol dave erin
mkchannel ops "$T_MAIN" private-ops "Private Ops" P "A private channel" "" alice bob carol
mkchannel crowd "$T_MAIN" crowd "Crowd" O "12+ members: more than the 10-collaborator cap" "" \
  alice bob "${CROWD[@]}"
mkchannel history "$T_MAIN" history "History" O "600 posts over 30 days: pagination and date separators" "" \
  sysadmin alice bob
mkchannel empty "$T_MAIN" empty "Empty" O "No posts" "" alice
mkchannel archived "$T_MAIN" archived "Archived" O "Archived channel" "" sysadmin alice bob
mkchannel second "$T_SECOND" second-general "Second General" O "Cross-team unreads and mentions" "" \
  alice bob

# ---------------------------------------------------------------------------------------------
step "Files and custom emoji"
python3 - "$OUT_DIR" <<'PY'
import struct, sys, zlib
def png(path, w, h, color):
    raw = b''.join(b'\x00' + bytes(color) * w for _ in range(h))
    chunk = lambda t, d: struct.pack('>I', len(d)) + t + d + struct.pack('>I', zlib.crc32(t + d))
    with open(path, 'wb') as f:
        f.write(b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('>IIBBBBB', w, h, 8, 2, 0, 0, 0))
                + chunk(b'IDAT', zlib.compress(raw)) + chunk(b'IEND', b''))
png(sys.argv[1] + '/emoji.png', 64, 64, (0x1e, 0x88, 0xe5))
png(sys.argv[1] + '/picture.png', 800, 450, (0x3d, 0xb8, 0x87))
PY
cat > "$OUT_DIR/notes.md" <<'EOF'
# Notes

A Markdown attachment for the file viewers.

- item one
- item two
EOF
cat > "$OUT_DIR/main.rs" <<'EOF'
fn main() {
    println!("Hello from a MatterLite attachment");
}
EOF

upload() { # token channel_id file -> file id
  curl -sS --fail-with-body -H "Authorization: Bearer $1" -F "channel_id=$2" -F "files=@$3" \
    "$API/files" | jq -r '.file_infos[0].id'
}
curl -sS --fail-with-body -o /dev/null -H "Authorization: Bearer $ADMIN" -F "image=@$OUT_DIR/emoji.png" \
  -F "emoji={\"creator_id\":\"${UID_OF[sysadmin]}\",\"name\":\"matterlite\"}" "$API/emoji"

# ---------------------------------------------------------------------------------------------
step "Posts"
# post TOKEN CHANNEL MESSAGE [ROOT_ID] [EXTRA_JSON] -> post id
post() {
  local body extra=${5:-}
  [[ -n "$extra" ]] || extra='{}'
  body=$(jq -nc --arg c "$2" --arg m "$3" --arg r "${4:-}" --argjson x "$extra" \
    '{channel_id:$c,message:$m,root_id:$r} + $x')
  api POST /posts "$1" "$body" | jq -r .id
}
react() { api POST /reactions "${TOKEN_OF[$1]}" "$(jq -nc --arg u "${UID_OF[$1]}" --arg p "$2" --arg e "$3" '{user_id:$u,post_id:$p,emoji_name:$e}')" >/dev/null; }

A=${TOKEN_OF[alice]} B=${TOKEN_OF[bob]} C=${TOKEN_OF[carol]} D=${TOKEN_OF[dave]}

post "$ADMIN" "${CH[town]}" "Welcome to the MatterLite test server. Users and passwords: see script/test-server/README.md in the MatterLite repo." >/dev/null
post "$B" "${CH[offtopic]}" "Anyone up for lunch? :pizza:" >/dev/null

# #dev: rendering showcase
SHOWCASE=$(cat <<'EOF'
# Heading 1
## Heading 2
### Heading 3

**bold**, *italic*, ~~strike~~, `inline code`, ***bold italic***, and a [link](https://mattermost.com).
Autolink: https://github.com/mattermost/mattermost · Emoji: :smile: :+1: :tada: :matterlite: · Channel: ~dev · #hashtag

> A blockquote
> spanning two lines

1. ordered
2. list
   - nested
   - bullets

- [ ] task
- [x] done task

| Left | Center | Right |
|:-----|:------:|------:|
| a    |   b    |     c |
| long cell content | x | 42 |

```rust
fn main() {
    println!("Hello, MatterLite!");
}
```

Inline math $e^{i\pi} + 1 = 0$ and a block:

```latex
\int_0^\infty e^{-x^2} dx = \frac{\sqrt{\pi}}{2}
```

---
Line one
Line two (soft break)
EOF
)
P_SHOWCASE=$(post "$A" "${CH[dev]}" "$SHOWCASE")
api POST "/posts/$P_SHOWCASE/pin" "$A" >/dev/null
react bob "$P_SHOWCASE" "+1"; react carol "$P_SHOWCASE" "+1"; react dave "$P_SHOWCASE" "tada"; react bob "$P_SHOWCASE" "matterlite"

P_ROOT=$(post "$B" "${CH[dev]}" "Thread root: should we ship the WebSocket resume fix today?")
post "$C" "${CH[dev]}" "Yes, the tests pass." "$P_ROOT" >/dev/null
post "$A" "${CH[dev]}" "Agreed. @bob can you tag the release?" "$P_ROOT" >/dev/null
P_REPLY=$(post "$B" "${CH[dev]}" "Tagged :rocket:" "$P_ROOT")
react alice "$P_REPLY" "white_check_mark"

FID_PNG=$(upload "$C" "${CH[dev]}" "$OUT_DIR/picture.png")
post "$C" "${CH[dev]}" "A screenshot" "" "{\"file_ids\":[\"$FID_PNG\"]}" >/dev/null
FID_MD=$(upload "$D" "${CH[dev]}" "$OUT_DIR/notes.md")
FID_RS=$(upload "$D" "${CH[dev]}" "$OUT_DIR/main.rs")
post "$D" "${CH[dev]}" "Two files for the viewers" "" "{\"file_ids\":[\"$FID_MD\",\"$FID_RS\"]}" >/dev/null

P_EDIT=$(post "$C" "${CH[dev]}" "This message has a typo: teh")
api PUT "/posts/$P_EDIT/patch" "$C" '{"message":"This message was edited: the"}' >/dev/null
P_DEL=$(post "$D" "${CH[dev]}" "This message gets deleted")
api DELETE "/posts/$P_DEL" "$D" >/dev/null
post "$B" "${CH[dev]}" "@alice please review the Markdown table rendering." >/dev/null
post "$B" "${CH[dev]}" "@here standup in 5 minutes" >/dev/null

# #collab
post "$A" "${CH[collab]}" "Use this channel to try shared drafts. Members: alice, bob, carol, dave, erin. outsider is on the team but not here." >/dev/null
P_COLLAB_ROOT=$(post "$B" "${CH[collab]}" "Thread for a shared reply draft")
post "$C" "${CH[collab]}" "First reply" "$P_COLLAB_ROOT" >/dev/null

# #private-ops, #crowd, #archived, second team
post "$C" "${CH[ops]}" "Private channel message for @alice" >/dev/null
post "$A" "${CH[crowd]}" "Crowd channel: alice, bob and crowd-01 … crowd-11." >/dev/null
post "$A" "${CH[archived]}" "Last message before archiving" >/dev/null
api DELETE "/channels/${CH[archived]}" "$ADMIN" >/dev/null
post "$B" "${CH[second]}" "@alice a mention in the second team" >/dev/null

# ---------------------------------------------------------------------------------------------
step "DMs and GM"
DM_AB=$(api POST /channels/direct "$A" "$(ids alice bob)" | jq -r .id)
DM_AC=$(api POST /channels/direct "$A" "$(ids alice carol)" | jq -r .id)
DM_SELF=$(api POST /channels/direct "$A" "$(ids alice alice)" | jq -r .id)
GM=$(api POST /channels/group "$A" "$(ids alice bob carol)" | jq -r .id)
post "$A" "$DM_AB" "Hi Bob!" >/dev/null
post "$B" "$DM_AB" "Hi Alice, got a minute?" >/dev/null
post "$B" "$DM_AB" "It's about the release notes." >/dev/null
post "$C" "$DM_AC" "Lunch tomorrow?" >/dev/null
post "$A" "$DM_SELF" "Note to self: test the self-DM" >/dev/null
post "$A" "$GM" "Group message for the three of us" >/dev/null
post "$C" "$GM" "Sounds good :smile:" >/dev/null

# ---------------------------------------------------------------------------------------------
step "History: 600 posts (580 backdated over 30 days, 20 unread after alice viewed)"
NOW_MS=$(($(date +%s) * 1000))
for i in $(seq 1 580); do
  ts=$((NOW_MS - (600 - i) * 72 * 60 * 1000))  # every 72 minutes
  msg="History message $i"
  (( i % 50 == 0 )) && msg="History message $i — a longer one: $(printf 'lorem ipsum dolor sit amet %.0s' {1..20})"
  post "$ADMIN" "${CH[history]}" "$msg" "" "{\"create_at\":$ts}" >/dev/null
done
api POST "/channels/members/${UID_OF[alice]}/view" "$A" "{\"channel_id\":\"${CH[history]}\"}" >/dev/null
for i in $(seq 581 600); do
  post "$ADMIN" "${CH[history]}" "History message $i (unread for alice)" >/dev/null
done

# ---------------------------------------------------------------------------------------------
step "Bot"
BOT_ID=$(api POST /bots "$ADMIN" '{"username":"testbot","display_name":"Test Bot","description":"Posts message attachments"}' | jq -r .user_id)
UID_OF[testbot]=$BOT_ID
BOT_TOKEN=$(api POST "/users/$BOT_ID/tokens" "$ADMIN" '{"description":"seed"}' | jq -r .token)
add_team_members "$T_MAIN" testbot
api POST "/channels/${CH[dev]}/members" "$ADMIN" "{\"user_ids\":$(ids testbot)}" >/dev/null
post "$BOT_TOKEN" "${CH[dev]}" "" "" '{"props":{"attachments":[{"fallback":"Build passed","color":"#3DB887","pretext":"CI report","title":"Build #42 passed","title_link":"https://example.com/builds/42","text":"All **312** tests passed.","fields":[{"title":"Branch","value":"main","short":true},{"title":"Duration","value":"4m 12s","short":true}]}]}}' >/dev/null
DM_BOT=$(api POST /channels/direct "$BOT_TOKEN" "$(ids alice testbot)" | jq -r .id)
post "$BOT_TOKEN" "$DM_BOT" "Hello from testbot. I'm a bot account." >/dev/null

# ---------------------------------------------------------------------------------------------
step "Statuses, favorites"
api PUT "/users/${UID_OF[bob]}/status" "$ADMIN" "{\"user_id\":\"${UID_OF[bob]}\",\"status\":\"dnd\"}" >/dev/null
api PUT "/users/${UID_OF[carol]}/status" "$ADMIN" "{\"user_id\":\"${UID_OF[carol]}\",\"status\":\"away\"}" >/dev/null
api PUT "/users/${UID_OF[dave]}/status/custom" "$D" '{"emoji":"palm_tree","text":"On vacation"}' >/dev/null
api PUT "/users/${UID_OF[alice]}/preferences" "$A" \
  "[{\"user_id\":\"${UID_OF[alice]}\",\"category\":\"favorite_channel\",\"name\":\"${CH[dev]}\",\"value\":\"true\"}]" >/dev/null

# ---------------------------------------------------------------------------------------------
step "MFA for frank"
F=${TOKEN_OF[frank]}
MFA_SECRET=$(api POST "/users/${UID_OF[frank]}/mfa/generate" "$F" | jq -r .secret)
api PUT "/users/${UID_OF[frank]}/mfa" "$F" "{\"activate\":true,\"code\":\"$(oathtool --totp -b "$MFA_SECRET")\"}" >/dev/null

# ---------------------------------------------------------------------------------------------
step "Access tokens"
ADMIN_PAT=$(api POST "/users/${UID_OF[sysadmin]}/tokens" "$ADMIN" '{"description":"plugin deploy"}' | jq -r .token)
ALICE_PAT=$(api POST "/users/${UID_OF[alice]}/tokens" "$ADMIN" '{"description":"matterlite tests"}' | jq -r .token)
BOB_PAT=$(api POST "/users/${UID_OF[bob]}/tokens" "$ADMIN" '{"description":"matterlite tests"}' | jq -r .token)

step "Deactivate user 'deactivated'"
api DELETE "/users/${UID_OF[deactivated]}" "$ADMIN" >/dev/null

# ---------------------------------------------------------------------------------------------
umask 077
cat > "$CREDS" <<EOF
# MatterLite test server credentials (generated by seed.sh $(date -Iseconds))
MM_SERVER_URL=$(mmctl --local config get ServiceSettings.SiteURL | tr -d '"')
MM_HTTPS_URL=https://ubuntu
MM_ADMIN_USER=sysadmin
MM_ADMIN_PASSWORD=$ADMIN_PASSWORD
MM_USER_PASSWORD=$PASSWORD
MM_ADMIN_TOKEN=$ADMIN_PAT
MM_ALICE_TOKEN=$ALICE_PAT
MM_BOB_TOKEN=$BOB_PAT
MM_BOT_TOKEN=$BOT_TOKEN
MM_FRANK_MFA_SECRET=$MFA_SECRET
EOF
step "Seeded. Credentials: $CREDS"
