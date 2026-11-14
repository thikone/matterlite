# MatterLite — collaborative drafts

A user can invite other people to see an unfinished message and edit it together in real time.
The person who started the draft sends the result, and it becomes an ordinary post from them.

This is a MatterLite feature, not a Mattermost one. It needs a small server plugin
(§5), and in the first version only MatterLite users can take part (§9). The editor
side builds on Warp's collaborative text buffer, which `ml_editor` already contains; the
parts that were removed during the extraction are listed in §8.

Related: [`MatterLite.md`](MatterLite.md) §13 (composer), [`EditorView.md`](EditorView.md)
(the editor), [`WebApp.md`](WebApp.md) §7 (WebSocket), §15 (drafts).

---

## Contents

1. [Goals](#1-goals)
2. [User experience](#2-user-experience)
3. [Rules](#3-rules)
4. [Why a plugin is needed](#4-why-a-plugin-is-needed)
5. [Architecture](#5-architecture)
6. [Protocol](#6-protocol)
7. [Client design](#7-client-design)
8. [What EditorView is missing and where it is in Warp](#8-what-editorview-is-missing-and-where-it-is-in-warp)
9. [Webapp and mobile users](#9-webapp-and-mobile-users)
10. [Security, privacy, compliance](#10-security-privacy-compliance)
11. [Testing](#11-testing)
12. [Plan](#12-plan)
13. [Open questions](#13-open-questions)

---

## 1. Goals

- **Write a message together** before it is posted: an announcement, a reply to a customer,
  release notes, a decision summary.
- **The result is a normal post** by the user who started the draft (the *owner*). Mentions,
  notifications, threads, reactions and edit history work as usual, in every client.
- **Live editing** with everyone's cursors visible, edits merged automatically, and undo
  limited to your own edits.
- **Nothing is posted** until the owner presses Send. Abandoned drafts disappear.

Not goals:

- **Editing a message after it is posted.** Mattermost lets only the author edit a post, and
  this feature does not change that.
- **Shared drafts as a server feature.** Mattermost drafts belong to one user and stay that way.
- **Webapp or mobile participation** in the first version (§9).

## 2. User experience

**Roles.** The *owner* starts the draft, invites people and is the only one who can send.
*Collaborators* edit, and can leave at any time.

**Flow:**

1. **Start.** In any composer (channel, DM, thread reply), the owner clicks **Write together**
   (or runs a command-palette entry) and picks people from the channel's members. The
   current composer text becomes the shared starting text, and the composer turns into the
   shared draft.
2. **Invite.** Each invitee gets a notice: "Alice invites you to write a message in
   #dev — Join / Decline". The notice appears as a toast and in the Activity tab under
   Drafts, so it isn't lost if missed.
3. **Join.** The collaborator's shared draft opens in the **right panel**, with:
   - the target channel and, for a thread, the root post;
   - the participants as avatar chips, showing who is present and who is still invited;
   - everyone's cursors and selections in their colours, with their avatar at the cursor.
4. **Edit.** Everyone types at once. Edits merge without conflicts, and each person's undo
   undoes only their own edits. The owner's composer shows the same live text.
5. **Send.** Only the owner has a **Send** button; collaborators see "Alice will send this".
   When the owner presses it:
   1. the draft is locked and becomes read-only for everyone ("Sending…");
   2. the owner's client waits until it has every edit made before the lock;
   3. it posts the text as the owner with the normal `POST /posts` (MatterLite.md §8, outbox);
   4. every participant sees "Sent" with a link to the post, and the shared draft closes.
6. **Cancel or leave.** The owner can **Stop collaborating**: the text stays in the owner's
   own composer as a normal draft, and collaborators see "Alice stopped the shared draft". A
   collaborator can **Leave**; their edits stay in the text.

**Attribution (optional).** The post can carry `props.co_authors: [user_id, …]`. The webapp
ignores unknown props; MatterLite can show "written with Bob and Carol" under the post.

**Presence.** A participant who closes the panel or goes offline shows as away in the
chips. Their cursor disappears, and their edits remain.

## 3. Rules

| Topic | Rule |
|---|---|
| Who can start | Anyone who can post in the channel (`create_post`; for a thread, also the root post must exist and not be locked) |
| Who can be invited | Members of the target channel; for a DM or GM, its participants. Inviting outsiders would show them a draft for a channel they can't read |
| How many | Up to 10 collaborators per draft (a setting on the plugin) |
| Who sends | The owner only. The post's `user_id` is the owner |
| Length | The server's `MaxPostSize` (client config). The editor's `max_buffer_len` enforces it for everyone |
| Attachments | The owner's uploads only: the server attaches a file only if the posting user uploaded it (`FileInfoStore.AttachToPost` checks `CreatorId`). The first version is text-only; adding the owner's own attachments is a later step |
| Owner goes offline | The draft pauses: collaborators can keep editing, but nobody can send. When the owner returns, their client catches up from the plugin's log (§6) |
| Expiry | A draft with no activity for 7 days is deleted by the plugin (a plugin setting) |
| One per owner and target | At most one shared draft per owner per channel or thread. Starting another one reopens it |

## 4. Why a plugin is needed

Mattermost gives clients no way to send custom data to each other:

- **Drafts are per user.** `model.Draft` has a single `UserId`. The `draft_created`,
  `draft_updated` and `draft_deleted` WebSocket events go only to that user's own sessions
  (`server/channels/app/draft.go`, `NewWebSocketEvent(…, dt.UserId, …)`).
- **Client WebSocket actions are fixed** (`server/channels/wsapi/*.go`):
  `authentication_challenge`, `ping`, `user_typing`, `presence`, `user_update_active_status`,
  `get_statuses`, `get_statuses_by_ids`, `posted_notify_ack`. `user_typing` carries only
  `channel_id` and `parent_id`.
- **Posts are the wrong transport.** Edits hidden in posts or post props would be stored
  forever, count toward unreads, show up as messages in other clients and hit rate limits.

Peer-to-peer connections (WebRTC data channels) would avoid the plugin. But they need
signalling through Mattermost anyway (posts or a plugin), a relay server (TURN) on most
corporate networks, and everyone online at once, and they bypass the server's audit. A plugin
is simpler and more reliable.

## 5. Architecture

```
┌──────────────┐  HTTPS: /plugins/<id>/api/v1/…   ┌─────────────────────────────────┐
│ MatterLite   │ ───────────────────────────────▶ │ Mattermost server               │
│ (owner)      │ ◀─────────────────────────────── │  ┌───────────────────────────┐  │
└──────────────┘  WebSocket: custom_<id>_* events │  │ plugin: collab relay      │  │
┌──────────────┐                                  │  │  • session + op log (KV)  │  │
│ MatterLite   │ ◀──────────────────────────────▶ │  │  • membership checks      │  │
│ (collaborator)│                                 │  │  • PublishWebSocketEvent  │  │
└──────────────┘                                  │  └───────────────────────────┘  │
                                                  │  POST /api/v4/posts (owner)     │
                                                  └─────────────────────────────────┘
```

**The plugin** (Go, server part only):

- **HTTP endpoints:** served through the `ServeHTTP` hook under `/plugins/<plugin_id>/`. The
  server authenticates the request and sets the `Mattermost-User-Id` header
  (`server/channels/app/plugin_requests.go`).
- **Storage:** the plugin's key-value store (`KVSetWithOptions`, `KVSetWithExpiry`) holds the
  session record and its operation log.
- **Relay:** `PublishWebSocketEvent(event, payload, broadcast)` sends events to clients. The
  server names them `custom_<plugin_id>_<event>` (`server/channels/app/platform/cluster.go`).
  - `WebsocketBroadcast` targets one user (`UserId`) or a whole channel (`ChannelId`), so the
    plugin publishes once per participant with `UserId` set.
  - It sets `OmitConnectionId` to skip the sender's own connection.
- **Permission checks:** `HasPermissionToChannel` and `GetChannelMember`.
- It never posts. The owner's client creates the post with the owner's own token, so the
  post is exactly like any other.

**Names.** Plugin id `matterlite-collab` (a placeholder until the plugin exists). Source in
`plugins/matterlite-collab/` in the MatterLite repository, built with Mattermost's plugin
toolchain.

**MatterLite** (§7):

- `ml_editor` with the collaboration code restored (§8);
- a new `ml_collab` crate holding the session model, the plugin's HTTP client and its
  WebSocket events;
- UI in the composer, the right panel and the Activity tab.

## 6. Protocol

### 6.1 Data

- **Session:**
  - `id`, `owner_id`, `channel_id`, `root_id` (empty for a channel post);
  - `base_text`, the owner's composer text at the start;
  - `participants` (invited, joined, left), `state` (`open`, `locked`, `sent`, `cancelled`);
  - `lock_seq`, `post_id`, `created_at`, `updated_at`.
- **Operation log:** an ordered list of `{seq, replica_id, user_id, ops}`. `seq` is assigned
  by the plugin. `ops` is a batch of Warp `Operation`s (`Edit`, `Undo`, `UpdateSelections`),
  which already derive serde `Serialize`/`Deserialize`. The wire format is JSON.
- **Replica id:** each client picks a random UUID per session and device
  (`ReplicaId::random`, §8). The plugin maps replica ids to users for presence and checks.

The order of the log doesn't matter to the editor: Warp's buffer accepts operations in any
order and holds back the ones whose dependencies haven't arrived. The `seq` exists for
catch-up ("everything after N") and to drop duplicates.

### 6.2 Endpoints (`/plugins/matterlite-collab/api/v1`)

| Method and path | Who | Effect |
|---|---|---|
| `GET /version` | anyone | Feature detection (404 → plugin not installed, hide the feature) |
| `POST /sessions` `{channel_id, root_id, base_text, invitees}` | owner | Creates the session, sends invites |
| `GET /sessions?mine=true` | anyone | Open sessions the user owns or is invited to (Activity tab, startup) |
| `GET /sessions/{id}` | participants | Session record |
| `POST /sessions/{id}/join`, `/leave`, `/decline` | invitees | Participant state |
| `POST /sessions/{id}/invite` `{user_ids}` | owner | More invitees |
| `POST /sessions/{id}/ops` `{replica_id, ops}` | joined participants | Appends a batch; returns its `seq`; rejected after the lock |
| `GET /sessions/{id}/ops?after={seq}` | participants | Catch-up |
| `POST /sessions/{id}/lock` | owner | `state = locked`, records `lock_seq` |
| `POST /sessions/{id}/sent` `{post_id}` | owner | `state = sent`, schedules deletion |
| `POST /sessions/{id}/unlock` | owner | Back to `open`, e.g. if posting failed |
| `DELETE /sessions/{id}` | owner | `state = cancelled`, deletes the log |

### 6.3 WebSocket events

All events are `custom_matterlite-collab_<name>`.

| Event | Sent to | Data |
|---|---|---|
| `invite` | the invitee | session summary (owner, channel, root post) |
| `ops` | the other joined participants | `{session_id, seq, replica_id, user_id, ops}` |
| `participants` | all participants | the participant list with states |
| `locked`, `unlocked` | all participants | `{session_id, lock_seq}` |
| `ended` | all participants | `{session_id, reason: sent|cancelled|expired, post_id?}` |

A client that misses events (for example after a reconnect, `WebApp §7.4`) calls
`GET /sessions/{id}/ops?after=<last seq>` before applying newer events.

### 6.4 Joining and catching up

1. Fetch the session and the log.
2. Create the buffer with the session's `base_text` and a new replica id.
   - Warp's buffer gives the starting text a fixed sentinel id (`ReplicaId::base_replica_id()`)
     and a deterministic fragment id (`FragmentId::between(min, max)`), in
     `Buffer::new_with_replica_id`.
   - So every copy built from the same `base_text` is identical.
3. Apply all batches from the log, then subscribe to `ops` events.

### 6.5 Sending

1. Owner: `POST /lock`. The plugin stops accepting `ops` and broadcasts `locked` with `lock_seq`.
2. Every client sets `InteractionState::Selectable`. Edits a collaborator made after the lock
   are rejected by the plugin. Their client shows "The draft was sent; your last change
   wasn't included".
3. The owner's client fetches and applies the log up to `lock_seq`, then posts the buffer text
   (`POST /api/v4/posts` with `channel_id`, `root_id` and optional `props.co_authors`).
4. On success: `POST /sent {post_id}`. On failure: show the error and either retry (the
   draft stays locked) or `POST /unlock`.

## 7. Client design

**`ml_collab` crate:**

- **Session model:** state, participants, last applied `seq`, pending outgoing batches, the
  lock state.
- **Plugin API client:** HTTP through `mm_api`'s client and token. Calls run on `mm_sync`'s
  Tokio runtime, and results come back over channels (MatterLite.md §6.1).
- **Events:** `mm_ws` passes `custom_matterlite-collab_*` events to it.
- **Batching:** outgoing operations are batched (for example every 50 ms or 32 operations)
  to limit requests; selection-only updates are coalesced.

**UI:**

- **Composer:** a "Write together" button and a participants strip in a decorator element
  (`EditorOptions::render_decorator_elements` → `top_section`).
- **Right panel:** the shared-draft view, using the same `EditorView` with remote cursors.
- **Invite notice:** a toast, plus Activity › Drafts entries for open invitations.
- **Picker:** channel members, with users who have MatterLite and the plugin first. The plugin
  can report which users connected with a MatterLite client recently.

**Drafts and the outbox.** While a shared draft is open, the owner's local draft for that
channel or thread (MatterLite.md §13) is not saved separately. On cancel it becomes the
local draft again. The final post goes through the normal outbox with its dedupe rules
(MatterLite.md §8.7).

## 8. What EditorView is missing and where it is in Warp

The extraction (see [`EditorView.md`](EditorView.md) §5) removed the collaboration half of
Warp's editor.

**Where to get it:**

- Everything is in Warp at `C:\Projects\Libs\warp-terminal` @ `5bb7bcc6a`, under
  `app/src/editor/view/`.
- It is also in MatterLite's verbatim import commit `79a4f9f`, under
  `crates/ml_editor/src/warp_import/editor/view/` (deleted from the tree by the next commit;
  `git show 79a4f9f:<path>`).

Line numbers below are in Warp's files.

### 8.1 Buffer: applying remote edits (`model/buffer/mod.rs`)

| Item | Warp location (lines) | Purpose |
|---|---|---|
| `Operation` enum (`Edit`, `Undo`, `UpdateSelections`) and `impl Operation` | L680 (12), L703 (33) | The unit sent between peers; serde-serializable |
| `UpdateSelectionsOperation` | L665 (14) | A peer's selections |
| `Buffer::apply_ops` | L1695 (26) | Applies a batch of remote operations; emits `Edited` with `EditOrigin::RemoteEdit` |
| `Buffer::apply_op` | L1722 (80) | Applies one remote edit, undo or selection update |
| `Buffer::apply_edit` | L1803 (121) | Inserts or deletes at fragment positions from a remote edit |
| `can_apply_op`, `flush_deferred_ops` | L1938 (22), L1925 (12) | Holds back operations whose dependencies haven't arrived |
| `resolve_fragment_id`, `split_fragment` | L1961 (17), L2834 (87) | Fragment lookup for remote edits |
| `deferred_ops` field, `mod deferred_ops` | `model/buffer/deferred_ops.rs` (69) | The holding queue |
| `end_batch` (operations part) | L943 (89) | Collects the batch's operations and emits `Event::UpdatePeers` |
| `record_selections_update` | L3167 (15) | Turns local selection changes into an operation |
| `Event::UpdatePeers { operations }` | in `pub enum Event`, L3265 | Hands local operations to the network layer |
| `EditOrigin::RemoteEdit` | `EditOrigin` enum | Marks edits that came from peers |
| `recreate(replica_id, base_text)` | L865 (6) | Recreates the buffer with a given replica id and starting text |
| `ReplicaId::random` using `uuid::Uuid::new_v4` | `model/buffer/time.rs` L21 (4) | MatterLite replaced it with a per-process counter; it must be a UUID again (add the `uuid` crate) |

### 8.2 Buffer and selections: remote cursors

| Item | Warp location (lines) | Purpose |
|---|---|---|
| `PeerSelectionData` `{colors, display_name, image_url, should_draw_cursors}`, `Peer` | `buffer/mod.rs` L114 (12), L127 (4) | How a peer's cursor is drawn |
| `remote_selections`, `registered_peers` fields | `struct Buffer` | Remote selections per replica; known peers |
| `remote_selections()`, `registered_peers()`, `register_peer`, `unregister_peer`, `unregister_all_peers`, `set_peer_selection_data` | `buffer/mod.rs` L880 (10), L1033–1064 (28) | Peer registry |
| `merge_remote_selections`, `merge_remote_selections_for_peer` | `buffer/mod.rs` L3099 (8), L3108 (9) | Keeps remote selections valid after edits |
| `RemoteSelection`, `RemoteSelections` and their impls | `model/selections.rs` L462–566 (~53) | Remote selection types |
| `RemoteDrawableSelectionData`, `DrawableSelection.replica_id` | `model/selections.rs` L35 (7) and `DrawableSelection` | Drawing data per peer |

### 8.3 Model and view API

| Item | Warp location (lines) | Purpose |
|---|---|---|
| `EditorModel::apply_remote_operations` | `model/mod.rs` L813 (25) | Applies remote operations to the regular buffer |
| `EditorModel::registered_peers`, `register_remote_peer`, `unregister_remote_peer`, `unregister_all_remote_peers`, `set_remote_peer_selection_data` | `model/mod.rs` L575–611 (~33) | Peer registry through the model |
| `EditorModel::recreate_buffer` | `model/mod.rs` L613 (13) | Starts a fresh collaborative buffer |
| `EditorModelEvent::UpdatePeers`, `handle_buffer_event_for_non_collaborative_editor` | `model/mod.rs`, L3025 (11) | Forwards local operations; skips them for temporary buffers |
| `all_drawable_selections_intersecting_range` (remote part) | `model/mod.rs` L2737 (22) | Adds remote selections to drawing |
| `EditorView::apply_remote_operations`, `register_remote_peer`, `unregister_remote_peer`, `unregister_all_remote_peers`, `set_remote_peer_selection_data`, `replica_id`, `reinitialize_buffer` | `view/mod.rs` L3292–3399 (~65) | Public API for the session layer |
| `Event::UpdatePeers`, `Event::BufferReinitialized` and the `UpdatePeers` arm of `handle_model_event` | `view/mod.rs` | Local operations out to the parent view |
| `all_drawable_selections_data` | `view/mod.rs` L7984 (66) | Builds per-peer colours and avatars for rendering |
| Display-only temporary buffer (`show_display_only_empty_buffer`, `exit_ephemeral_loading_state`) | `model/mod.rs` L627–650, `view/mod.rs` | Optional: a read-only "viewer" mode |

For `reinitialize_buffer`, MatterLite needs a variant that also takes the starting text,
because the session starts from `base_text` (§6.4). `Buffer::recreate` already takes it.

### 8.4 Rendering

| Item | Warp location (lines) | Purpose |
|---|---|---|
| `RemoteDrawableSelectionData` (element version) and its `From` impl | `view/element.rs` L165 (8), L174 (9) | Avatar converted to an element |
| `is_local_replica` | `view/element.rs` L264 (4) | |
| Remote parts of `draw_selections_and_get_cursor_data` | `view/element.rs` L914 (295 in total) | Draws each peer's selections in their colours |
| Remote parts of `draw_cursors`, `REMOTE_BEAM_CURSOR_WIDTH_PX` | `view/element.rs` L820 (73) | Thinner remote cursors with avatar bubbles |
| Avatar layout in `layout` | `view/element.rs` | |
| `cursor_avatar_font_size`, `cursor_avatar_size`, `CURSOR_AVATAR_*` | `view/snapshot.rs` L639, L645 (10) | Avatar sizing |
| `Avatar`, `AvatarContent` | `app/src/ui_components/avatar.rs` (254) | The avatar bubble (image or initials). Never imported into MatterLite; use it or MatterLite's own avatar component |

### 8.5 Size and tests

- **Size:** restoring all of this is about 1,000 lines in `EditorView.rs`, plus the
  254-line avatar component.
- **Tests:** Warp's CRDT tests are the main safety net and should come along. They're in
  `app/src/editor/view/model/buffer/mod_tests.rs` (4,458 lines) with helpers in
  `model/buffer/test.rs` (89). They were never imported into MatterLite. The relevant ones:
  - `test_concurrent_insertions`, `test_concurrent_insertions_at_same_location`;
  - `test_concurrent_non_overlapping_deletions`, `test_concurrent_overlapping_deletions`,
    `test_concurrent_replace`;
  - `test_multiple_peers`, `test_random_concurrent_operations`;
  - `test_remote_undo_redo`, `test_concurrent_edit_with_undo`,
    `test_concurrent_edit_with_conflicting_undo`, `test_concurrent_deletions_with_undo`;
  - `test_undo_should_not_restore_remote_selection`, `test_merging_selections_with_remote_edits`.

### 8.6 How Warp wired it (reference only)

Warp's shared sessions are the closest working example of the glue code:

- **Outgoing:** `app/src/terminal/input.rs` handles `EditorEvent::UpdatePeers` (~L11123) by
  storing the operations (`latest_buffer_operations`) and forwarding them to Warp's
  session-sharing server.
- **Incoming:** `process_remote_edits` (~L15024) applies incoming operations, holding them
  back until the matching command block is active.
- **Joining:** `on_session_share_joined` (~L11876) calls `reinitialize_buffer(Some(replica_id))`.
- **Presence:** `app/src/terminal/view/shared_session/view_impl.rs` registers and unregisters
  peers as viewers join and leave, and updates their cursor data from Warp's presence manager
  (`register_remote_peer`, `set_remote_peer_selection_data`, ~L414, L1223, L1651).

## 9. Webapp and mobile users

The first version is MatterLite-only: the picker marks users who can't join, and invites to
them are not sent.

Possible later step: a webapp part in the plugin with a compatible editor. The buffer depends
only on `sum_tree`, `string-offset` and a few `warpui` text types, so compiling it to
WebAssembly and wrapping it in a plain text area is plausible. Warp's UI crates once had a
wasm target, which MatterLite removed. Mobile would need the same in React Native. Both are
large and not planned.

## 10. Security, privacy, compliance

- **Explicit invites only**, and only channel members (§3). The plugin checks membership on
  every request, not only at invite time, so someone who leaves the channel loses access.
- **Data at rest:** the draft text and its edit log live in the plugin's key-value store until
  the draft is sent or cancelled, then are deleted (plus the inactivity expiry). Admins
  should know this; the plugin's description says so.
- **Audit and compliance export:** they see the final post as the owner's post. Who typed
  what isn't recorded, apart from the optional `co_authors` prop.
- **No server-side posting:** the plugin never acts as a user. The post is created with the
  owner's token.
- **Size limits:** the plugin caps the log per session (for example 5 MB) and the batch size
  per request, to protect the key-value store.

## 11. Testing

- **Editor:** port Warp's CRDT tests (§8.5) into `EditorView_tests.rs`, or a separate
  `EditorView_crdt_tests.rs` if it gets too large.
- **Session layer:** in-process tests with two or three `EditorView`s and a fake plugin
  that delivers operations late, duplicated and in random order. Check that all copies end
  up with the same text and that the lock-then-send step loses nothing typed before the lock.
- **Plugin:** Go unit tests for permission checks, `seq` assignment, lock rejection and expiry.
- **End to end:** a local Mattermost with the plugin, two MatterLite instances. Covered by the
  manual test list in MatterLite.md §17.

## 12. Plan

The feature is optional and comes after the MVP and threads (MatterLite.md §18, M9). Steps:

1. **Editor:** restore §8.1 and §8.3 (minus the display-only buffer) as
   `port(warp): collaborative editing for ml_editor`. Bring back the UUID replica ids, then the
   CRDT tests. Update the `ml_editor` PROVENANCE row and `EditorView.md`.
2. **Remote cursors:** restore §8.2 and §8.4, using Mattermost avatars from the avatar cache.
3. **Plugin:** session store, endpoints, events, expiry.
4. **`ml_collab`:** session model, catch-up, batching, lock-then-send.
5. **UI:** composer button and strip, right-panel view, invite toast, Activity entries,
   attribution under posts.
6. **Later:** the owner's attachments, an optional read-only viewer role, webapp support (§9).

## 13. Open questions

- **Should collaborators see the draft in their own composer** instead of the right panel?
  The right panel keeps their own composer free for other messages.
- **Should a thread reply's collaborators get the root post loaded** if they haven't opened
  the thread yet? Probably yes, through the normal permalink flow.
- **Is `co_authors` shown in MatterLite only,** or should the plugin also add a small "edited
  together with …" line that other clients can see? A visible line changes the post text,
  which is surprising, so props-only is the default.
- **Plugin distribution:** a separate release artifact signed for Mattermost's plugin
  marketplace, or upload-only for self-hosted servers?
