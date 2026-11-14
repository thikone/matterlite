# mm_model fixtures

Two kinds of JSON samples for the serde tests.

## Hand-written (`*.json` here)

`channels.json` and `post_list.json` are written by hand from the Go structs in the server's
`server/public/model`. They cover what a real response rarely shows: `null` for nil slices,
maps and pointers, `omitempty` fields left out, and fields from newer servers that the model
must ignore. Keep them for those cases (`src/*_tests.rs`).

## Captured (`captured/`)

Real responses from the test server (`docs/MatterLite-TestServer.md`): Mattermost 12.2.0,
seeded by `script/test-server/seed.sh`, fetched as `alice` with her token. Tested by
`tests/captured.rs`, which checks the seeded data (§5 of that document).

| File | Request |
|---|---|
| `config_client.json`, `license_client.json` | `GET /config/client?format=old`, `GET /license/client?format=old` |
| `user_me.json`, `preferences.json` | `GET /users/me`, `GET /users/me/preferences` |
| `teams.json`, `team_members.json`, `team_unreads.json` | `GET /users/me/teams`, `.../teams/members`, `.../teams/unread?include_collapsed_threads=true` |
| `channels.json` | `GET /users/me/channels` |
| `channel_members.ndjson` | `GET /users/{id}/channel_members?page=-1` (NDJSON, as streamed) |
| `sidebar_categories.json` | `GET /users/{id}/teams/{team}/channels/categories` (team MatterLite) |
| `dev_posts.json` | `GET /channels/{dev}/posts?per_page=60&before=…&collapsedThreads=true` (the seeded `#dev` posts) |
| `thread_posts.json` | `GET /posts/{root}/thread` (the `#dev` thread) |
| `threads.json` | `GET /users/{id}/teams/{team}/threads?extended=true` |
| `roles.json` | `POST /roles/names` (`system_user`, `team_user`, `channel_user`) |
| `users.json`, `statuses.json` | `POST /users/ids`, `POST /users/status/ids` (bob, carol, dave) |
| `emoji.json` | `GET /emoji` |
| `ws_frames.jsonl` | Every frame alice's WebSocket received while bob posted, edited, reacted, typed and replied, and alice followed a thread, muted a channel, collapsed a category and changed a preference (`RUST_LOG=mm_ws::frames=trace mm-cli … events`) |

To capture again after a reset and seed, send the requests above with
`Authorization: Bearer $MM_ALICE_TOKEN` and pipe JSON through `jq .`. Ids and timestamps change
on every seed; the tests look things up by name. Check that no token or secret from
`credentials.env` ends up in a file. Two values were replaced after capture: the server's
`DiagnosticId`/`TelemetryId` in `config_client.json` and `server_hostname` in the `hello`
frame.
