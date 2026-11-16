# MatterLite — test server

How to test MatterLite against a real Mattermost: the server on the Ubuntu VM, its accounts and
data, the expected starting state, how to drive live events, and how to deploy the
`matterlite-collab` plugin (`MatterLite-Collaborate.md`). Protocol details are in `WebApp.md`
(`WebApp §N`); the general test plan is `MatterLite.md` §17.

The scripts that build and seed the server are in `script/test-server/`.

---

## Contents

1. [Quick facts](#1-quick-facts)
2. [Topology](#2-topology)
3. [First steps from Windows](#3-first-steps-from-windows)
4. [Accounts](#4-accounts)
5. [Test data](#5-test-data)
6. [Expected starting state](#6-expected-starting-state)
7. [What to test where](#7-what-to-test-where)
8. [Driving live events](#8-driving-live-events)
9. [Server configuration](#9-server-configuration)
10. [Operating the server](#10-operating-the-server)
11. [Plugins](#11-plugins)
12. [HTTPS and WSS](#12-https-and-wss)
13. [Limitations](#13-limitations)
14. [Troubleshooting](#14-troubleshooting)

---

## 1. Quick facts

| | |
|---|---|
| Server URLs | `https://ubuntu` (TLS through nginx, §12) and `http://ubuntu:8065` (plain, straight to Mattermost) |
| REST / WebSocket | `https://ubuntu/api/v4`, `wss://ubuntu/api/v4/websocket`; plain: `http://ubuntu:8065/api/v4`, `ws://ubuntu:8065/api/v4/websocket` |
| Web UI | Either URL; the official webapp built from the same commit, for side-by-side comparison |
| TLS trust | Certificate from the "MatterLite Test CA", trusted machine-wide on Windows (§12) |
| Version | Mattermost **12.2.0**, Team Edition, unlicensed, from `mattermost/mattermost` @ `5b9537164d` (same commit as `C:\Projects\Libs\mattermost`) |
| Passwords | All users `matterlite`; admin `sysadmin` / `Sys@dmin-sample1` |
| Main account | `alice` (in every channel, with unreads and mentions waiting) |
| Tokens, MFA secret | `ssh andrew@ubuntu cat mm-test-server/credentials.env`; they change on every seed |
| Reset | `reset.sh`, then `seed.sh` (§10) |

## 2. Topology

```
Windows host                                  Ubuntu 26.04 VM (VMware), ssh andrew@ubuntu
┌──────────────────────────┐  HTTPS/WSS :443  ┌──────────────────────────────────────────────┐
│ MatterLite, mm_cli,      │ ───────────────▶ │ nginx (TLS, HTTP/2; :80 redirects to https)  │
│ browser (web UI)         │                  │   │ proxy                                    │
│                          │  HTTP/WS :8065   │   ▼                                          │
│ hosts: 192.168.199.129   │ ───────────────▶ │ systemd mattermost-dev :8065                 │
│        ubuntu            │                  │   ~/Projects/mattermost/server/bin/mattermost│
│ trusts: MatterLite       │                  │   client -> ../webapp/channels/dist          │
│         Test CA          │                  │ PostgreSQL 18 (localhost:5432)               │
└──────────────────────────┘                  │ mmctl --local (/var/tmp/mattermost_local.socket)
                                              └──────────────────────────────────────────────┘
```

- **Why a VM:** Windows here can't run WSL2 or Docker (virtualization is off), and the server's
  dev tooling expects Linux.
- **Host name:** clients must use `ubuntu`, the name in the Windows hosts file. The VM calls
  itself `andrew-ubuntu`, which doesn't resolve from Windows. On the VM, `ubuntu` maps to
  `127.0.1.1`. The server's `SiteURL` is `http://ubuntu:8065` for both URLs (§12).
- **On the VM:**

  | What | Where |
  |---|---|
  | Source checkout (shallow, one commit) | `~/Projects/mattermost` |
  | Binaries | `server/bin/mattermost`, `server/bin/mmctl` (also `/usr/local/bin/mmctl`) |
  | Config | `server/config/config.json` |
  | Uploaded files | `server/data/` |
  | Plugins | `server/plugins/`, `server/client/plugins/` |
  | Server log | `server/mattermost.log` (JSON lines), `journalctl -u mattermost-dev` |
  | nginx | `/etc/nginx/sites-available/mattermost`; logs in `/var/log/nginx/` |
  | TLS | CA `~/mm-test-ca/` (kept by `reset.sh`); server certificate and key `/etc/ssl/matterlite/` |
  | Seed output (credentials, sample files) | `~/mm-test-server/` |
  | Go | `/usr/local/go` (1.26.10) |
  | Node for the webapp build only | `~/.local/opt/node-v24.11.1-linux-x64` (the webapp needs exactly 24.11.1; system Node is untouched) |

## 3. First steps from Windows

In Git Bash (has `curl` and `jq`):

```sh
# Server up? (Git Bash curl uses Windows' Schannel: see §12 for --ssl-revoke-best-effort)
curl -s http://ubuntu:8065/api/v4/system/ping | jq .status                         # "OK"
curl -s --ssl-revoke-best-effort https://ubuntu/api/v4/system/ping | jq .status    # "OK"

# Load the credentials into the shell (MM_SERVER_URL, MM_HTTPS_URL, MM_*_TOKEN, MM_FRANK_MFA_SECRET, ...)
eval "$(ssh andrew@ubuntu cat mm-test-server/credentials.env)"

# Token works?
curl -s -H "Authorization: Bearer $MM_ALICE_TOKEN" $MM_SERVER_URL/api/v4/users/me | jq -r .username
```

The protocol harness (`crates/mm_cli`):

```sh
cargo run -p mm_cli -- https://ubuntu ping
MM_PASSWORD=matterlite cargo run -p mm_cli -- https://ubuntu bootstrap --login alice
MM_TOKEN=$MM_ALICE_TOKEN cargo run -p mm_cli -- https://ubuntu events    # wss://ubuntu/api/v4/websocket
cargo run -p mm_cli -- http://ubuntu:8065 ping                          # plain HTTP, same server
# MFA: the code is read from stdin when there is no terminal
ssh andrew@ubuntu "oathtool --totp -b $MM_FRANK_MFA_SECRET" | MM_PASSWORD=matterlite cargo run -p mm_cli -- https://ubuntu bootstrap --login frank
# raw WebSocket frames
RUST_LOG=mm_ws::frames=trace MM_TOKEN=$MM_ALICE_TOKEN cargo run -p mm_cli -- https://ubuntu events
# the sync engine with a cache under ./mlsync; commands on stdin (open dev, show dev, send dev hi,
# older dev, newer dev, close, unread, search words, quit). Run it again to start from the cache.
MM_TOKEN=$MM_ALICE_TOKEN cargo run -p mm_cli -- https://ubuntu sync --data mlsync
```

Use a token, not a password login, for `sync` runs you want to restart from the cache: a
password session `mm-cli` created is logged out when it exits.

In MatterLite, add the server as `https://ubuntu` (or `http://ubuntu:8065`) and sign in as
`alice` / `matterlite`. Both URLs reach the same server and data.

## 4. Accounts

| User | Name | Role and state | Use it for |
|---|---|---|---|
| `sysadmin` | — | System admin; password `Sys@dmin-sample1`; token `MM_ADMIN_TOKEN`. Created every channel, so it is a member of all of them and of both teams | Admin API calls, plugin deploys, backdated posts |
| `alice` | Alice Anderson ("Ali") | Member of everything; token `MM_ALICE_TOKEN` | The main account: bootstrap, unreads, mentions, rendering |
| `bob` | Bob Brown | Status **dnd**; token `MM_BOB_TOKEN` | The second party for live events (§8) |
| `carol` | Carol Chen | Status **away** | A third party; GM member |
| `dave` | Dave Davis | Custom status `:palm_tree:` "On vacation" | Custom status rendering |
| `erin` | Erin Evans | Plain member | Extra collaborator |
| `frank` | Frank Fischer | **MFA on**; secret `MM_FRANK_MFA_SECRET` | The MFA login flow (`WebApp §3.3`) |
| `outsider` | Olga Outsider | On team MatterLite, but not in `#collab` or `#dev` | Permission checks: may not be invited to shared drafts |
| `loner` | Lars Loner | In no team | Bootstrap with zero teams |
| `deactivated` | Dee Activated | Deactivated | Login refused (`api.user.login.inactive.app_error`); deactivated authors in member lists |
| `crowd-01`…`crowd-11` | Crowd Member NN | Members of `#crowd` | Large member lists; going past the 10-collaborator cap |
| `testbot` | Test Bot | Bot account, owned by sysadmin; token `MM_BOT_TOKEN` | Bot badges, message attachments, bot DMs |

E-mail addresses are `<username>@example.com`. All users can log in by username or e-mail.

**MFA code for frank.** On the VM: `oathtool --totp -b "$MM_FRANK_MFA_SECRET"`. On Windows, any
TOTP tool with the base32 secret (SHA-1, 6 digits, 30 s). A password-only login returns
`mfa.validate_token.authenticate.app_error`; send the code in the login body's `token` field.

## 5. Test data

### Team "MatterLite" (`matterlite`, open)

Every channel below also has `sysadmin` as a member (it created them).

| Channel | Type | Members besides sysadmin | Content |
|---|---|---|---|
| `town-square` | Open | everyone on the team | Welcome post by sysadmin |
| `off-topic` | Open | everyone on the team | One post by bob (`:pizza:`) |
| `dev` | Open | alice, bob, carol, dave, erin, frank, testbot | Rendering showcase (below); alice's **favorite** |
| `collab` | Open | alice, bob, carol, dave, erin | For shared drafts: an intro post, and a thread root by bob with one reply by carol |
| `private-ops` | **Private** | alice, bob, carol | "Private channel message for @alice" |
| `crowd` | Open | alice, bob, crowd-01…11 | One post; 14 members in all |
| `history` | Open | alice, bob | 600 posts by sysadmin: #1–580 every 72 min over ~29 days (backdated), #581–600 now. Every 50th post is long |
| `empty` | Open | alice | No posts (only system messages) |
| `archived` | Open, **archived** | alice, bob | One post, then archived |

Header of `#dev`: `Rendering showcase · see **pinned** posts · [Mattermost](https://mattermost.com)`.

**`#dev` posts, oldest first:**

1. **Markdown showcase** by alice, **pinned**:
   - headings h1–h3, bold, italic, strikethrough, bold italic, inline code, a link, an autolink;
   - emoji `:smile: :+1: :tada:`, the custom emoji `:matterlite:`, the channel link `~dev`, `#hashtag`;
   - a blockquote, ordered and nested lists, a task list, an aligned table;
   - a `rust` code block, inline LaTeX, a `latex` block, a horizontal rule and soft line breaks.

   Reactions: 👍 ×2 (bob, carol), 🎉 (dave), `:matterlite:` (bob).
2. **Thread:** a root by bob, with replies by carol, alice ("@bob can you tag the release?") and
   bob ("Tagged :rocket:", ✅ reaction by alice). Alice follows it (she replied).
3. **Image** by carol: `picture.png`, 800×450, with thumbnail and preview.
4. **Two files** by dave: `notes.md` and `main.rs`.
5. **Edited post** by carol: "This message was edited: the" (`edit_at` set).
6. **Deleted post** by dave: gone from the timeline.
7. "@alice please review the Markdown table rendering." (bob).
8. "@here standup in 5 minutes" (bob).
9. **Bot post** by testbot: empty message with a message attachment (pretext, title link,
   colour bar, Markdown text, two short fields).

### Team "Second Team" (`second`, invite only)

Alice, bob and sysadmin. `town-square`, `off-topic`, and `second-general` with "@alice a
mention in the second team" by bob.

### Direct and group messages

| Channel | Messages |
|---|---|
| alice ↔ bob | alice "Hi Bob!", then two from bob |
| alice ↔ carol | one from carol |
| alice ↔ alice (self-DM) | one note from alice |
| alice ↔ testbot | one from the bot |
| GM alice, bob, carol | one from alice, one from carol |

### Other

- **Custom emoji** `:matterlite:` (a blue square, by sysadmin).
- **Sample files** on the VM, in `~/mm-test-server/`: `picture.png`, `emoji.png`, `notes.md`,
  `main.rs`.
- **Access tokens:** for sysadmin, alice, bob and testbot.

## 6. Expected starting state

What `mm_cli bootstrap --login alice` prints right after a seed. Use it as the reference for
MatterLite's sidebar badges; the official web UI should show the same.

```
CRT        server AlwaysOn, preference (not set): on
teams (2)
  matterlite   MatterLite    32 unread, 6 mentions; threads 1 unread, 0 mentions
  second       Second Team   1 unread, 3 mentions; threads 0 unread, 0 mentions

channels (16, 17 memberships)
  [MatterLite]
    ~collab               1 unread
    ~crowd
    ~dev                  8 unread, 2 mentions
    ~empty                1 mentions
    ~history              20 unread
    ~off-topic            1 unread, 1 mentions
    ~private-ops          1 unread, 2 mentions
    ~town-square          1 unread
  [Second Team]
    ~off-topic            1 mentions
    ~second-general       1 unread, 2 mentions
    ~town-square
  [Direct and group messages]
    @alice (online)
    @bob (dnd)            2 unread, 2 mentions
    @carol (away)         1 unread, 1 mentions
    @testbot (online)     1 unread, 1 mentions
    alice, bob, carol     1 unread, 1 mentions

sidebar categories
  [MatterLite] Favorites (1) · Channels (7) · Direct Messages (5)
  [Second Team] Favorites (0) · Channels (3) · Direct Messages (5)
```

After this, `bootstrap` prints the sidebar as `mm_store` derives it (`sidebar (mm_store)`):
each team's badge with "matches the server", the categories with their visible channels, and
the DM/GM show preferences the webapp would save. Alice's self-DM is not listed: it is read
and has no `direct_channel_show` preference, so the webapp hides it too (`WebApp §14`).

Some numbers look odd but are what the server does (`WebApp §10`, `§12`):

- **Collapsed Reply Threads is on.** Channel counts are root posts only (`msg_count_root`,
  `mention_count_root`).
  - For `#dev`, the plain `GET /users/me/channels/{id}/unread` returns `msg_count` 11. The
    root-only count is 8.
  - "threads 1 unread" is the `#dev` thread: alice replied, so she follows it, and bob replied
    after her.
- **"added to the channel" counts as a mention, but not as an unread message.** The system
  post "alice added to the channel by sysadmin." (`system_add_to_channel`) mentions alice.
  - That's the mention in `#empty` and in the second-team `off-topic`, and one of the two in
    `#private-ops` and `#second-general`.
  - In `#crowd`, alice was added in one batch with many users, and that message doesn't
    mention her.
- **DM and GM messages count as mentions.** Each one adds to that channel's mention count.
  The team totals don't include them: MatterLite's 6 = `#dev` 2 + `#empty` 1 + `off-topic` 1 +
  `#private-ops` 2.
- **`#history`:** alice viewed the channel after post 580 (`POST /channels/members/{alice}/view`),
  so 20 are unread. The "new messages" line goes above post 581.
- **Statuses:** bob's `dnd` and carol's `away` were set manually, so they stay. Everyone else is
  online only while connected; testbot is reported as online.
- **"16 channels, 17 memberships":** the difference is `#archived`. The channel list leaves out
  archived channels by default; the membership list doesn't.

## 7. What to test where

| Feature (`WebApp §`) | Where | What to check |
|---|---|---|
| Ping, client config, license (§6) | anonymous, then alice | `IsLicensed` `false`; the logged-in client config has `CollapsedThreads` `always_on` and `MaxPostSize` |
| Password login, by username and e-mail (§3.2) | `alice` / `alice@example.com` | session token from the `Token` response header |
| MFA (§3.3) | `frank` | password-only → `mfa.validate_token.authenticate.app_error`; then with the code |
| Personal access token (§3.1) | `MM_ALICE_TOKEN` | `Authorization: Bearer`; WebSocket `authentication_challenge` |
| Wrong password, deactivated user (§3.2) | `alice` + bad password, `deactivated` | error ids, no retry storm |
| Logout, revoked session (§3.5) | revoke alice's sessions as sysadmin (§8) | 401 → back to sign-in |
| Bootstrap with no teams (§6) | `loner` | empty state, no crash |
| Teams, team switch, cross-team badges (§14) | alice: MatterLite, Second Team | mention badge on the inactive team |
| Sidebar categories, favorites (§14) | alice | Favorites (1) holds `#dev` |
| Private and archived channels | `#private-ops`, `#archived` | lock icon; archived is read-only |
| Unreads, mentions, CRT (§10) | §6 above | badges match; viewing a channel clears them |
| Timeline pagination (§9) | `#history` | 600 posts; `before`/`after` paging; date separators over 29 days; new-messages line |
| Markdown rendering (§11) | `#dev` showcase | every element in §5 |
| Custom emoji | `:matterlite:` in the showcase and a reaction | image loads from `/emoji/{id}/image` |
| Threads (§9.4) | `#dev` thread, `#collab` thread | reply counts, participants, followed threads |
| Reactions (§9.4) | showcase post | grouping, counts, own reaction highlighted |
| Edited and deleted posts (§9.4) | `#dev` | "(edited)"; the deleted post absent |
| Files and previews | `#dev` image and files | thumbnail, preview, download, `FileInfo` |
| Message attachments, bots (§17) | testbot post in `#dev`, bot DM | BOT badge, attachment layout |
| System messages (§12) | join/add messages in every channel | rendering and grouping |
| DMs, GM, self-DM (§14) | alice's DMs | display names, self-DM label |
| Presence, custom status (§15) | bob dnd, carol away, dave custom | icons and texts |
| Large member lists | `#crowd` (14) | member list, mention autocomplete |
| Live events, resume (§7) | §8 | events applied; seq resume; resync on a new connection id |
| Collaborative drafts | `#collab`, `outsider`, `#crowd` | invites, permission rejection, the 10-collaborator cap (`MatterLite-Collaborate.md`) |

## 8. Driving live events

Act as another user while MatterLite (or `mm_cli events`) is connected as alice. In Git Bash,
after loading the credentials (§3):

```sh
API=$MM_SERVER_URL/api/v4
as_bob() { curl -s -H "Authorization: Bearer $MM_BOB_TOKEN" -H 'Content-Type: application/json' "$@"; }
BOB=$(as_bob $API/users/me | jq -r .id)
ALICE=$(as_bob $API/users/username/alice | jq -r .id)
DEV=$(as_bob $API/teams/name/matterlite/channels/name/dev | jq -r .id)

# posted
P=$(as_bob -X POST $API/posts -d "{\"channel_id\":\"$DEV\",\"message\":\"live message for @alice\"}" | jq -r .id)
# post_edited, reaction_added/removed, post_deleted
as_bob -X PUT  $API/posts/$P/patch -d '{"message":"edited live"}' >/dev/null
as_bob -X POST $API/reactions -d "{\"user_id\":\"$BOB\",\"post_id\":\"$P\",\"emoji_name\":\"heart\"}" >/dev/null
as_bob -X DELETE $API/users/$BOB/posts/$P/reactions/heart >/dev/null
as_bob -X DELETE $API/posts/$P >/dev/null
# typing
as_bob -X POST $API/users/$BOB/typing -d "{\"channel_id\":\"$DEV\"}" >/dev/null
# bob's status (seeded dnd; put it back afterwards). The server sends status_change only to
# bob's own connections, so alice sees it by polling POST /users/status/ids (WebApp §15)
as_bob -X PUT $API/users/$BOB/status -d "{\"user_id\":\"$BOB\",\"status\":\"online\"}" >/dev/null
as_bob -X PUT $API/users/$BOB/status -d "{\"user_id\":\"$BOB\",\"status\":\"dnd\"}" >/dev/null
# DM to alice
DM=$(as_bob -X POST $API/channels/direct -d "[\"$BOB\",\"$ALICE\"]" | jq -r .id)
as_bob -X POST $API/posts -d "{\"channel_id\":\"$DM\",\"message\":\"ping\"}" >/dev/null
```

**What MatterLite sends.** `script/test-server/listen.mjs` prints the WebSocket events another
user receives (Node 22+ has `WebSocket` built in; the VM has Node 24), e.g. alice's typing and
posts as bob sees them, with the seconds since the start. `typing.mjs` sends `user_typing` every
2 s over the WebSocket, like a real client:

```sh
scp script/test-server/*.mjs andrew@ubuntu:mm-test-server/tools/
ssh andrew@ubuntu 'source ~/mm-test-server/credentials.env; MM_TOKEN=$MM_BOB_TOKEN node ~/mm-test-server/tools/listen.mjs 20 typing posted'
ssh andrew@ubuntu 'source ~/mm-test-server/credentials.env; MM_TOKEN=$MM_BOB_TOKEN node ~/mm-test-server/tools/typing.mjs <channel id> 8'
```

This snippet was run against the server: every call succeeds. The same works for carol and the
others: log in by password (`POST /users/login`, `Token` header) or create a token with the
admin token (`POST /users/{id}/tokens`).

**Reconnects and resume (`WebApp §7.4`, §7.5).**

| Goal | How |
|---|---|
| A short drop, then resume with `connection_id` and `sequence_number` | Disconnect the client briefly (pause it, or disable the VM's network adapter in VMware), post a few messages as bob, reconnect. The server keeps the dropped connection for about 5 minutes and queues up to **256** events for it (`sendQueueSize`; the 128-event `deadQueueSize` holds frames already sent), `server/channels/app/platform/web_conn.go` |
| A gap too large to resume | While alice is disconnected, send more than 256 events, e.g. `for i in $(seq 300); do as_bob -X POST $API/posts -d "{\"channel_id\":\"$DEV\",\"message\":\"burst $i\"}" >/dev/null; done`. Reconnecting gets a new connection id; the client must resync |
| Server restart (a new connection id, a short outage) | `ssh andrew@ubuntu sudo systemctl restart mattermost-dev` |
| A long outage (backoff) | `ssh andrew@ubuntu sudo systemctl stop mattermost-dev`, then `start` |
| Session revoked (401 on REST; the WebSocket stays open but answers requests with `not_authenticated`) | `curl -X POST -H "Authorization: Bearer $MM_ADMIN_TOKEN" $API/users/$ALICE/sessions/revoke/all`. This revokes password sessions only; `MM_ALICE_TOKEN` keeps working |
| Keep the client offline (for the gap above) | On the VM: `sudo nft add table inet mltest; sudo nft add chain inet mltest input '{ type filter hook input priority -10; }'; sudo nft add rule inet mltest input ip saddr 192.168.199.1 tcp dport 443 reject with tcp reset; sudo ss -K state established '( sport = :443 )' dst 192.168.199.1`. Undo: `sudo nft delete table inet mltest`. A short drop without blocking: only the `ss -K` part |

## 9. Server configuration

`setup.sh` changes these from the defaults (patched into `config.json`, so they stay editable
in the System Console):

| Setting | Value | Why |
|---|---|---|
| `ServiceSettings.SiteURL` | `http://ubuntu:8065` | Links, WebSocket origin |
| `ServiceSettings.EnableLocalMode` | `true` (from the environment, so read-only in the System Console) | `mmctl --local` without credentials |
| `EnableUserAccessTokens`, `EnableBotAccountCreation` | `true` | Tokens for tests, `testbot` |
| `EnableMultifactorAuthentication` | `true` (not enforced) | `frank` |
| `EnableCustomEmoji`, `EnableLatex`, `EnableInlineLatex`, `EnableLinkPreviews` | `true` | Rendering tests |
| `EnableIncomingWebhooks`, `EnableOutgoingWebhooks`, `EnableCommands`, `EnablePostUsernameOverride`, `EnablePostIconOverride` | `true` | Integrations, overridden names and icons |
| `TeamSettings.EnableOpenServer` | `true` | Sign-up without an invite |
| `TeamSettings.MaxUsersPerTeam` | `1000` | Room for load data |
| `EmailSettings.SendEmailNotifications`, `RequireEmailVerification`, `EnablePreviewModeBanner` | `false` | No SMTP on the VM |
| `PluginSettings.Enable`, `EnableUploads` | `true` | Plugin deploys |
| `PluginSettings.RequirePluginSignature`, `EnableMarketplace` | `false` | Unsigned dev plugins; no outbound marketplace calls |
| `RateLimitSettings.Enable` | `false` | Bursts in tests |

Defaults worth knowing:

| Setting | Value |
|---|---|
| `ServiceSettings.CollapsedThreads` | `always_on` |
| `ServiceSettings.ThreadAutoFollow` | `true` |
| Session lengths (web, mobile) | 4320 h (180 days) |
| `ServiceSettings.MaximumPayloadSizeBytes` | 300000 |
| `FileSettings.MaxFileSize` | 100 MB; local storage `./data/` |
| `PasswordSettings.MinimumLength` | 8, no character classes |
| `TeamSettings.RestrictDirectMessage` | `any` |
| `TeamSettings.TeammateNameDisplay` | `username` |
| `MaxPostSize` | in the logged-in client config; typically 16383 (`WebApp §9`) |
| Search | database (no Elasticsearch/OpenSearch) |

Read or change a setting on the VM: `mmctl --local config get ServiceSettings.CollapsedThreads`,
`mmctl --local config set <key> <value>`. Most changes apply without a restart.

## 10. Operating the server

From the MatterLite repository on Windows:

| Task | Command |
|---|---|
| Install, build, start (idempotent; `REBUILD=1` forces builds) | `ssh andrew@ubuntu 'bash -s' < script/test-server/setup.sh` |
| Seed (no-op if already seeded) | `ssh andrew@ubuntu 'bash -s' < script/test-server/seed.sh` |
| Wipe the DB, files and plugins (config kept) | `ssh andrew@ubuntu 'bash -s' < script/test-server/reset.sh` |
| Credentials | `ssh andrew@ubuntu cat mm-test-server/credentials.env` |
| M2 acceptance: the sync engine end to end (about 2 minutes; `RESET=1` resets and seeds first) | `script/test-server/m2-acceptance.sh` |
| Status, restart | `ssh andrew@ubuntu systemctl status mattermost-dev`, `sudo systemctl restart mattermost-dev` |
| Follow the log | `ssh andrew@ubuntu journalctl -u mattermost-dev -f`, or `tail -f ~/Projects/mattermost/server/mattermost.log` |
| nginx | `ssh andrew@ubuntu sudo nginx -t`, `sudo systemctl reload nginx`, `sudo tail -f /var/log/nginx/error.log` |
| Admin CLI | `ssh andrew@ubuntu mmctl --local <command>` (e.g. `user list`, `channel list matterlite`) |
| SQL | `ssh andrew@ubuntu psql postgres://mmuser:mostest_password@localhost/mattermost_test` |

- **Reset and seed** take well under a minute (the seed alone about 16 s).
- **The seed is deterministic** (same names, posts and order), but ids, timestamps and tokens
  are new each time. Tests should look things up by name, not hard-code ids.
- **Another Mattermost version:** `MM_COMMIT=<sha> ssh …setup.sh` fetches that commit and
  rebuilds the server and the web UI. Database migrations run on start.
  Going back to an older commit needs `reset.sh`.
- **Overrides:** `setup.sh` also takes `SITE_URL` and `MM_DIR`; `seed.sh` takes `PASSWORD` and
  `ADMIN_PASSWORD`.
- **Real Mattermost behaviour:** the official webapp at `http://ubuntu:8065` is the reference.
  Log in as a different user than MatterLite, or the same user to see how both clients handle
  shared state (read marks, preferences).

## 11. Plugins

Prepared for `matterlite-collab` (`MatterLite-Collaborate.md` §5, §11): plugins and uploads on,
signatures not required, marketplace off. No plugins are enabled after a reset.

| Task | How |
|---|---|
| Deploy from Windows (plugin starter template) | `MM_SERVICESETTINGS_SITEURL=http://ubuntu:8065 MM_ADMIN_TOKEN=$MM_ADMIN_TOKEN make deploy` |
| Deploy with the REST API | `curl -H "Authorization: Bearer $MM_ADMIN_TOKEN" -F plugin=@dist/<bundle>.tar.gz -F force=true $API/plugins`, then `POST $API/plugins/<id>/enable` |
| Deploy on the VM | `mmctl --local plugin add <bundle>.tar.gz --force && mmctl --local plugin enable <id>` |
| Build on the VM | Go 1.26.10 is installed; the plugin's Go module builds against `github.com/mattermost/mattermost/server/public` |
| Plugin log | `server/mattermost.log`, filter by `"plugin_id":"matterlite-collab"` |
| KV store | `select pkey, length(pvalue), expireat from pluginkeyvaluestore where pluginid = 'matterlite-collab';` |
| Endpoints | `https://ubuntu/plugins/matterlite-collab/api/v1/...` (or via `:8065`) with the user's token; the server adds `Mattermost-User-Id` |
| Events | `custom_matterlite-collab_<event>` on the normal WebSocket |

**Two-client tests:** run two MatterLite instances (different data directories), alice and bob,
both members of `#collab`. Use `outsider` for rejected invites and `#crowd` for the cap.

## 12. HTTPS and WSS

nginx on the VM terminates TLS and proxies to Mattermost on `127.0.0.1:8065`, like a production
install (Mattermost's recommended nginx configuration). Port 8065 stays open for plain HTTP.

| | |
|---|---|
| URLs | `https://ubuntu`, `wss://ubuntu/api/v4/websocket`; also `https://andrew-ubuntu`, `https://192.168.199.129`, `https://localhost` (on the VM) |
| Port 80 | `301` redirect to `https://` |
| Protocols | TLS 1.2 and 1.3; HTTP/2 (ALPN `h2`) and HTTP/1.1. The WebSocket always uses HTTP/1.1 |
| Certificate | ECDSA P-256, `CN=ubuntu`, SANs `ubuntu`, `andrew-ubuntu`, `localhost`, `192.168.199.129`, `127.0.0.1`; valid 397 days (to Dec 2027) |
| Issuer | `CN=MatterLite Test CA, O=MatterLite` (ECDSA P-256, valid to Nov 2036), thumbprint (SHA-1) `3721FCADFE509DE4DEBF06E2B16C1612B041D2EA` |
| Revocation | None published (no CRL or OCSP) |
| WebSocket timeouts | `proxy_read_timeout 90s` (the server pings every 60 s); REST `600s` |
| Upload limit | `client_max_body_size 110M` (above `MaxFileSize` 100 MB) |
| Proxy headers | `Host`, `X-Forwarded-For`, `X-Real-IP`, `X-Forwarded-Proto`; Mattermost trusts `X-Forwarded-For` and `X-Real-IP` (`TrustedProxyIPHeader`) |

**The CA is name-constrained** (critical `nameConstraints`). It can only issue for `ubuntu`,
`andrew-ubuntu`, `localhost` and the private ranges `192.168.0.0/16`, `10.0.0.0/8`,
`127.0.0.0/8`. Trusting it can't make any other site trusted, even if its key leaked. The key
is on the VM only (`~/mm-test-ca/ca.key`).

**Trust on Windows.** The CA is installed in `Cert:\LocalMachine\Root`, so MatterLite, `mm_cli`,
Edge/Chrome, .NET/PowerShell and Git Bash `curl` trust it for every account. MatterLite checks
certificates through the OS store: reqwest's rustls platform verifier, and
`rustls-platform-verifier` for the WebSocket (`crates/mm_ws/src/tls.rs`).

```powershell
# install (elevated); the file comes from: scp andrew@ubuntu:mm-test-ca/ca.crt matterlite-test-ca.crt
certutil -addstore Root matterlite-test-ca.crt
# check
Get-ChildItem Cert:\LocalMachine\Root | Where-Object Subject -like '*MatterLite Test CA*'
# remove (elevated)
certutil -delstore Root 3721FCADFE509DE4DEBF06E2B16C1612B041D2EA
```

Other machines: Linux `sudo cp ca.crt /usr/local/share/ca-certificates/matterlite-test-ca.crt
&& sudo update-ca-certificates` (the VM itself is set up this way); macOS
`sudo security add-trusted-cert -d -r trustRoot -k /Library/Keychains/System.keychain ca.crt`.

**Revocation.** The certificate has no CRL or OCSP address, so Windows reports "revocation
status unknown".

- **Tolerant (works):** MatterLite (`rustls-platform-verifier` sets
  `CERT_CHAIN_POLICY_IGNORE_ALL_REV_UNKNOWN_FLAGS`), browsers, and .NET.
- **Strict (fails):** Git Bash `curl` (Schannel) fails by default with exit 35, or 60 with
  `--cacert`. Pass `--ssl-revoke-best-effort`.

**SiteURL stays `http://ubuntu:8065`.** Mattermost has one `SiteURL`; it is used in links the
server generates (permalinks, file links, e-mail), not for routing.

- **WebSocket URL:** MatterLite builds it from the URL the user entered
  (`mm_api::ServerUrl::websocket_url`, with `WebsocketURL` empty), so `https://ubuntu` gives
  `wss://ubuntu/api/v4/websocket`.
- **Making HTTPS the canonical address:** `SITE_URL=https://ubuntu` with `setup.sh`, or
  `mmctl --local config set ServiceSettings.SiteURL https://ubuntu`.

**Renewal.** `setup.sh` reissues the server certificate when it's missing, expires within 30
days, or doesn't cover the VM's current IP address. Re-run `setup.sh` after VMware assigns a
new address. The CA is kept, so Windows doesn't need to trust anything new.

**What it lets you test:**

- `https`/`wss` connections with HTTP/2 for REST;
- the behaviour behind a reverse proxy: nginx closes a WebSocket that is silent for 90 s. The
  server's 60 s pings keep healthy connections open, so a drop there points at a stalled
  connection;
- large uploads through the proxy;
- the `http://ubuntu` → `https://` redirect, if MatterLite follows redirects when a user
  types a bare host.

TLS failures to test:

| Failure | How |
|---|---|
| Untrusted certificate | Remove the CA temporarily (`certutil -delstore Root …`) and connect to `https://ubuntu` |
| Name mismatch | Add another name for 192.168.199.129 to the Windows hosts file (e.g. `mm-wrong`) and connect to `https://mm-wrong` |

Neither is set up permanently.

## 13. Limitations

- **Team Edition, unlicensed.** No guest accounts, LDAP, SAML, announcement banners,
  compliance export, read-only channels, data retention, shared channels or other licensed
  features.
- **No SSO.** GitLab, Google, Office 365 and OpenID aren't configured; only e-mail/username
  login works (`WebApp §3.4` can't be tested here).
- **No e-mail.** Password reset and e-mail invites don't arrive (no SMTP). `RequireEmailVerification`
  is off.
- **No push notifications** (no push proxy).
- **Private CA only.** No publicly trusted certificate (the VM isn't reachable from the
  internet), and no CRL or OCSP (§12).
- **Single node.** No cluster or high-availability behaviour.

## 14. Troubleshooting

| Problem | Fix |
|---|---|
| `ubuntu` doesn't resolve | Check `C:\Windows\System32\drivers\etc\hosts` has `192.168.199.129 ubuntu`. If VMware gave the VM a new address, update it (and `SITE_URL` if the name changes) |
| Connection refused on 8065 | `ssh andrew@ubuntu systemctl status mattermost-dev`; then `journalctl -u mattermost-dev -n 100` |
| `Already seeded` | Run `reset.sh` first |
| 401 with a saved token | Tokens are replaced on every seed: reload `credentials.env` |
| MFA code rejected | The VM clock is off; check `timedatectl` on the VM (TOTP allows only a small skew) |
| WebSocket closes at once | Wrong `Origin` header: native clients should send none, or one whose host matches the URL they connect to |
| `https://ubuntu`: certificate not trusted | Check the CA is installed (§12). A new VM or a recreated `~/mm-test-ca` means a new CA: install the new one, remove the old |
| `https://ubuntu`: name or IP mismatch | The VM got a new IP: update the hosts file and re-run `setup.sh` (reissues the certificate) |
| curl exit 35 or 60 on Windows | Schannel's strict revocation check: add `--ssl-revoke-best-effort` (§12) |
| 502 Bad Gateway on https | nginx is up but Mattermost isn't: `systemctl status mattermost-dev` |
| 413 on upload through https | Above nginx's `client_max_body_size` (110M); use `:8065` or raise it in `/etc/nginx/sites-available/mattermost` |
| Web UI blank after a source update | Rebuild the web UI: `REBUILD=1 … setup.sh` |
| Disk full | `df -h /` on the VM; Go caches are in `~/go` and `~/.cache/go-build` |
