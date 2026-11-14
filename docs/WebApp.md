# How the Mattermost Webapp Works — Notes for a Native Rust Client

This document explains how the React webapp in this repository talks to the Mattermost
server and what it computes on the client side. It is written for someone building a
**native client in Rust** that must behave like the webapp: same protocol, same data model,
same unread/notification semantics, same message rendering rules.

Everything here was derived from the source in this repo. File paths are relative to the
repository root. Abbreviations used throughout:

| Abbrev | Path |
|---|---|
| `PC` | `webapp/platform/client/src` — `@mattermost/client` (REST + WebSocket client) |
| `PT` | `webapp/platform/types/src` — `@mattermost/types` (TypeScript API/store types) |
| `MR` | `webapp/channels/src/packages/mattermost-redux/src` — redux store, actions, selectors |
| `CS` | `webapp/channels/src` — the Channels web application |
| `SM` | `server/public/model` — Go API models, Go REST client, Go WebSocket client |

Line numbers are indicative (they drift as the code changes); function names are stable.

---

## Table of contents

1. [TL;DR — the things you must get right](#1-tldr--the-things-you-must-get-right)
2. [Repository map](#2-repository-map)
3. [Authentication](#3-authentication)
4. [REST conventions](#4-rest-conventions)
5. [Endpoint cheat-sheet](#5-endpoint-cheat-sheet)
6. [Bootstrap sequence](#6-bootstrap-sequence)
7. [WebSocket protocol](#7-websocket-protocol)
8. [Data model and client state](#8-data-model-and-client-state)
9. [Posts and the timeline](#9-posts-and-the-timeline)
10. [Unreads, mentions and Collapsed Reply Threads](#10-unreads-mentions-and-collapsed-reply-threads)
11. [Message rendering](#11-message-rendering)
12. [System messages and post types](#12-system-messages-and-post-types)
13. [Notifications](#13-notifications)
14. [Channel sidebar](#14-channel-sidebar)
15. [Typing, presence, statuses, drafts](#15-typing-presence-statuses-drafts)
16. [Permissions](#16-permissions)
17. [Slash commands, integrations and plugins](#17-slash-commands-integrations-and-plugins)
18. [Routing and deep links](#18-routing-and-deep-links)
19. [Local persistence](#19-local-persistence)
20. [Suggested Rust architecture](#20-suggested-rust-architecture)
21. [Key source files index](#21-key-source-files-index)

---

## 1. TL;DR — the things you must get right

1. **Auth with a Bearer token, not cookies.** `POST /api/v4/users/login` returns the session
   token in the **`Token` response header**. Send it as `Authorization: Bearer <token>` on
   every request. CSRF checks apply only to cookie-authenticated non-GET requests, so a
   Bearer-only client never needs `X-CSRF-Token`.
2. **Always send `X-Requested-With: XMLHttpRequest`.** The webapp does; some server paths
   (e.g. login cookie handling, error rendering) behave differently without it.
3. **Client config + license are flat `map<string,string>`** (`/config/client?format=old`,
   `/license/client`; the server ignores `format`). Booleans are the strings `"true"`/`"false"`. Many features are gated
   by them (CRT, LaTeX, synced drafts, teammate name display, …).
4. **The WebSocket is sequenced.** Every server event carries `seq`; you must see exactly
   `expected_seq` each time or drop the connection and reconnect. Reconnect with
   `?connection_id=…&sequence_number=…` to get missed events replayed. If `hello` returns a
   **different** `connection_id`, events were lost: reset seq to 0 and do a full resync.
5. **Many WebSocket payload fields are JSON-encoded strings** (`data.post`, `data.reaction`,
   `data.channel`, `data.thread`, `data.preferences`, `data.mentions`, …). Parse twice.
6. **Unread counts are computed on the client**: `channel.total_msg_count - member.msg_count`
   (or the `_root` variants when Collapsed Reply Threads is on). Mentions come from
   `member.mention_count(_root)`. Muted = `notify_props.mark_unread == "mention"`.
7. **Collapsed Reply Threads (CRT)** changes which counters apply, whether replies appear in
   the channel timeline, and adds a separate Threads API with its own unread counts.
8. **Optimistic posting** uses `pending_post_id = "{user_id}:{epoch_ms}"`. The server echoes
   it back in the created post and in the `posted` WebSocket event; replace the local
   pending post by matching it.
9. **Post order is by `create_at` descending**; post lists come back as
   `{order: [ids], posts: {id: Post}, next_post_id, prev_post_id}` and must be merged into
   contiguous blocks.
10. **Mark channels read** with `POST /channels/members/me/view` when the user views them,
    otherwise unread counts never clear (also on other devices).
11. **Desktop notifications are decided by the client** (`shouldSkipNotification` logic,
    §13). The server just tells you who was mentioned.
12. **Markdown is a GFM dialect** (fork of `marked`) with Mattermost extensions: mentions,
    `~channel` links, emoji shortcodes and emoticons, hashtags, inline/block LaTeX, image
    size syntax. HTML is sanitized (raw HTML not rendered).
13. **System messages are rendered client-side** from `post.type` + `post.props`
    (the `message` field is a fallback English string).
14. **Consecutive join/leave system posts are combined client-side** into one virtual post.
15. **Sidebar categories come from the server** (`/channels/categories`) and must be kept in
    sync via `sidebar_category_*` events; DM auto-close is client logic.

---

## 2. Repository map

```
api/v4/source/*.yaml          OpenAPI 3 sources (≈58 files). `make build` in api/ concatenates
                              them into api/v4/html/static/mattermost-openapi-v4.yaml (not committed).
                              introduction.yaml documents auth, rate limits, errors, WebSocket.
server/public/model/          Go API models + reference clients:
    client4.go                  Go REST client (good reference for every endpoint)
    websocket_client.go         Go WebSocket client (auth challenge, reliable reconnect)
    websocket_message.go        All WebSocket event names + envelope types
    post.go, channel.go, user.go, team.go, thread.go, ... JSON shapes (serde reference)
server/channels/api4/         HTTP handlers (routes, params, permission checks)
server/channels/app/platform/ WebSocket hub/conn (web_conn.go, web_hub.go, websocket_router.go)
server/channels/wsapi/        WebSocket client actions handled by the server
webapp/                       npm workspaces monorepo (React 19, TypeScript, Webpack 5)
    platform/types/             @mattermost/types — TS types for API & store   (PT)
    platform/client/            @mattermost/client — Client4 REST + WebSocketClient (PC)
    platform/components, shared Shared React UI/utilities (irrelevant for Rust)
    channels/                   The Channels web app (CS)
        src/packages/mattermost-redux/src/   Redux store, actions, reducers, selectors (MR)
        src/actions/            App-level thunks (websocket_actions.ts, new_post.ts, notifications)
        src/components/         React UI
        src/utils/              Markdown, text formatting, emoji, constants
        src/i18n/*.json         33 locale files (react-intl message ids)
```

**Which source to use for serde structs?**

- For **wire format**, the Go structs in `SM` are authoritative (`json:"…"` tags,
  `omitempty`). Use them to decide which fields are optional.
- For **what the client actually reads**, `PT/*.ts` is more convenient
  (`posts.ts`, `channels.ts`, `users.ts`, `teams.ts`, `threads.ts`, `preferences.ts`,
  `config.ts`, `channel_categories.ts`, `store.ts` for the whole state shape).
- Make every struct tolerant: `#[serde(default)]` everywhere, unknown fields ignored, and
  keep `props`/`metadata` as `serde_json::Value` (or a typed struct with a `#[serde(flatten)]
  extra` map). Servers add fields constantly.

---

## 3. Authentication

### 3.1 How the server finds the token

`server/channels/app/authentication.go` (`ParseAuthTokenFromRequest`), in order:

1. `MMAUTHTOKEN` cookie (wins even when an `Authorization` header is present)
2. `Authorization: Bearer <token>`
3. `Authorization: Token <token>` (OAuth access tokens)
4. `?access_token=<token>` query parameter

CSRF (`server/channels/web/handlers.go`) is enforced only when the token came from the
cookie and the method is not GET. **Recommendation for a native client: never store or send
cookies; use the Bearer header only.**

### 3.2 Password login

```http
POST /api/v4/users/login
X-Requested-With: XMLHttpRequest
Content-Type: application/json

{"login_id": "alice", "password": "…", "token": "<mfa code or empty>", "device_id": "<push device id or empty>"}
```

- Success: `200`, body = sanitized `User`; **session token in the `Token` response header**
  (`server/channels/app/login.go`). Cookies (`MMAUTHTOKEN` HttpOnly, `MMUSERID`, `MMCSRF`)
  are set only when `X-Requested-With: XMLHttpRequest` is present — ignore them.
- `ldap_only: "true"` restricts to LDAP.
- Rate-limited (5 req/s, burst 10, `server/channels/api4/user.go`).
- Webapp: `PC/client4.ts` `login()`; Go: `SM/client4.go` `Login`/`LoginWithDevice`.
- Personal access tokens (`/users/{id}/tokens`) also work as Bearer tokens — handy for
  development and bots.

### 3.3 MFA

When the user has MFA and the code is missing/invalid, login fails with
`mfa.validate_token.authenticate.app_error` (400) or
`api.user.check_user_mfa.bad_code.app_error` (401). The webapp then shows the MFA input and
**re-sends the same login request with `token` filled** (`CS/components/login/login.tsx`).
`api.context.mfa_required.app_error` (403) on later calls means MFA is enforced but not set
up for this user. All other login failures are masked as
`api.user.login.invalid_credentials_*`.

### 3.4 SSO (GitLab/Google/Office365/OpenID/SAML)

SSO is browser based. The mobile flow is the easiest for a native app:

- `GET /oauth/{service}/mobile_login?redirect_to=<your-scheme://callback>`
- `GET /login/sso/saml?action=mobile&redirect_to=<your-scheme://callback>`

On success the server redirects to `redirect_to` with `MMAUTHTOKEN` and `MMCSRF` as query
parameters (`server/channels/web/oauth.go`, `server/channels/web/saml.go`). Open the URL in
a system browser or embedded webview and capture the redirect (custom URL scheme or
loopback server). The desktop app uses a different handoff
(`/users/login/desktop_token`); the PKCE code-exchange endpoint is deprecated.

Use `GET /config/client` keys (`EnableSignInWithEmail`, `EnableSignInWithUsername`,
`EnableLdap`, `EnableSaml`, `EnableSignUpWithGitLab`, `EnableSignUpWithGoogle`,
`EnableSignUpWithOffice365`, `EnableSignUpWithOpenId`, …) to decide which login options to show.

### 3.5 Logout and expiry

- `POST /users/logout` revokes the session.
- An expired/revoked session returns `401` with id `api.context.session_expired.app_error`
  (`server/channels/web/context.go`). Treat any 401 on an authenticated call as "logged out".
- **The WebSocket is not closed on revocation** (checked against 12.2). The socket stays open;
  the server stops sending events and answers every request (including `ping`) with
  `{"status":"FAIL","error":{"id":"api.web_socket_router.not_authenticated.app_error",
  "status_code":401}}` (`platform/websocket_router.go`). A client that ignores failed ping
  replies keeps a silent socket until the session's original expiry. `mm_ws` treats that
  reply as `AuthFailed`.
- The webapp broadcasts logout across tabs via a localStorage key; a native client should
  just wipe its token and cache.

---

## 4. REST conventions

### 4.1 Base URL and headers

- Base: `{siteUrl}/api/v4` (`PC/client4.ts` `getBaseRoute`). Support a site URL with a
  subpath (e.g. `https://example.com/mattermost`).
- Request headers (`PC/client4.ts` `getOptions`):
  - `X-Requested-With: XMLHttpRequest` — always
  - `Authorization: Bearer <token>`
  - `Content-Type: application/json` when there is a JSON body
  - `Accept-Language`, `User-Agent` — optional
  - `Connection-Id: <ws connection_id>` — sent on some calls (e.g. file upload) so the server
    can attribute the action to your WebSocket connection
- Useful response headers: `X-Version-Id` (stored as `serverVersion`), `X-Cluster-Id`,
  `Token` (login), rate-limit headers (below).
  - `X-Version-Id` is `<version>.<build number>.<client config hash>.<licensed>`
    (`server/channels/web/handlers.go`), e.g. `12.2.0.123.a16e….false`. A build from source
    has an empty build number (`12.2.0..a16e….false`). `hello.server_version` has the same
    format. A changed config hash means the client config changed (`mm_model::ServerVersion`).

### 4.2 Response bodies

`PC/client4.ts` `doFetchWithResponse` picks a parser by Content-Type:

- `application/json` — normal case
- `application/x-ndjson` — one JSON object per line (e.g. streaming channel members)
- zip / octet-stream / csv — binary
- anything else — text

### 4.3 Errors

Error bodies are `AppError` (`SM/utils.go`):

```json
{"id": "api.context.session_expired.app_error", "message": "Invalid or expired session, please login again.",
 "detailed_error": "", "request_id": "…", "status_code": 401}
```

Branch on `id`, not on `message` (which is localized). **Exception:** `429 Too Many Requests`
has a plain-text body ("limit exceeded"), not JSON. Rate-limit headers:
`X-RateLimit-Limit`, `X-RateLimit-Remaining`, `X-RateLimit-Reset`, `Retry-After`
(`server/channels/app/ratelimit.go`).

### 4.4 Pagination

- `page` is 0-based; `per_page` defaults to 60, max 200 (`server/channels/web/params.go`).
- Posts additionally support `before=<post_id>`, `after=<post_id>`, `since=<epoch_ms>`.
- Threads support `before`/`after` (thread ids), `since`, `per_page`.
- `GET /users/{id}/channel_members?page=-1` returns **all** memberships as NDJSON stream.
- **A user in no channel gets a 404, not an empty list**, from both streamed endpoints
  (checked against 12.2; `api4/channel.go` `getChannelsForUser`, `api4/user.go`
  `getChannelMembersForUser`):
  - `GET /users/me/channels`: `404` with the body `[` followed by the `AppError`
    `app.channel.get_channels.not_found.app_error` (the `[` is written before the first page
    is queried);
  - `GET /users/{id}/channel_members?page=-1`: `404` `app.channel.get_member.missing.app_error`.

  Treat both as empty lists; `mm_api` does.

### 4.5 ETag / 304

The server sets `ETag` on several GETs (channel posts without `since`, post thread, file
infos, single user, users in team, channels) and returns `304 Not Modified` for a matching
`If-None-Match`. The webapp doesn't use this, but a native client with a local cache can.

### 4.6 Timestamps and IDs

- All timestamps are **milliseconds since epoch** (`i64`). `0` means "not set"
  (e.g. `delete_at == 0` → not deleted, `edit_at == 0` → never edited).
- IDs are 26-char lowercase base32 strings (`[a-z0-9]{26}`).

---

## 5. Endpoint cheat-sheet

All paths below are under `/api/v4`. Function names refer to `PC/client4.ts` (the Go
equivalents in `SM/client4.go` have similar names).

### 5.1 Bootstrap / system

| Method | Path | Purpose | Client4 |
|---|---|---|---|
| GET | `/system/ping` | Health check (unauthenticated) | `ping` |
| GET | `/config/client?format=old` | Client config (flat string map) | `getClientConfig` |
| GET | `/license/client` | License info (flat string map) | `getClientLicenseOld` |
| GET | `/users/me` | Current user | `getMe` |
| GET | `/users/me/preferences` | All preferences | `getMyPreferences` |
| GET | `/users/me/teams` | My teams | `getMyTeams` |
| GET | `/users/me/teams/members` | My team memberships | `getMyTeamMembers` |
| GET | `/users/me/teams/unread?include_collapsed_threads=true` | Per-team unread/mention counts | `getMyTeamUnreads` |
| POST | `/roles/names` | Body `["system_user","team_user",…]` → roles with permissions | `getRolesByNames` |

### 5.2 Channels and membership

| Method | Path | Purpose |
|---|---|---|
| GET | `/users/me/channels` | All my channels in all teams (incl. DMs/GMs) |
| GET | `/users/me/teams/{team_id}/channels` | My channels in a team (DMs/GMs included) |
| GET | `/users/{user_id}/channel_members?page=-1` | All my channel memberships (NDJSON) |
| GET | `/users/me/teams/{team_id}/channels/members` | My memberships in a team |
| GET | `/channels/{id}` | One channel |
| GET | `/teams/{team_id}/channels/name/{name}` | Channel by name |
| GET | `/teams/name/{team}/channels/name/{name}` | Channel by team name + name |
| POST | `/channels/direct` | Body `[my_id, other_id]` → DM channel (creates if needed) |
| POST | `/channels/group` | Body `[ids…]` → GM channel |
| GET | `/channels/{id}/members/me` | My membership |
| GET | `/channels/{id}/stats` | Member count, pinned count, … |
| POST | `/channels/{id}/members` | Join/add user `{user_id}` |
| DELETE | `/channels/{id}/members/{user_id}` | Leave/remove |
| PUT | `/channels/{id}/members/{user_id}/notify_props` | Mute, notify level, … |
| POST | `/channels/members/me/view` | **Mark viewed**: `{channel_id, prev_channel_id, collapsed_threads_supported: true}` |
| POST | `/channels/members/me/mark_read` | Mark several channels read (body: array of ids) |
| GET | `/teams/{team_id}/channels?page&per_page` | Browse public channels |
| GET/PUT/POST | `/users/{uid}/teams/{tid}/channels/categories` (+ `/order`, `/{category_id}`) | Sidebar categories |

### 5.3 Posts

| Method | Path | Purpose |
|---|---|---|
| GET | `/channels/{id}/posts?page&per_page` | Latest page |
| GET | `/channels/{id}/posts?before=<post_id>&per_page` | Older page |
| GET | `/channels/{id}/posts?after=<post_id>&per_page` | Newer page |
| GET | `/channels/{id}/posts?since=<ms>` | Everything changed since (sync) |
| GET | `/users/{uid}/channels/{cid}/posts/unread?limit_before=30&limit_after=30` | Window around first unread |
| *(common params)* | `collapsedThreads=true`, `collapsedThreadsExtended=true`, `skipFetchThreads=true` | CRT behaviour |
| POST | `/posts` | Create (201). `{channel_id, message, root_id?, file_ids?, props?, metadata?: {priority}, pending_post_id}` |
| PUT | `/posts/{id}/patch` | Partial update `{message?, is_pinned?, file_ids?, props?, has_reactions?}` |
| PUT | `/posts/{id}` | Full update |
| DELETE | `/posts/{id}` | Delete |
| GET | `/posts/{id}` | One post |
| GET | `/posts/{id}/thread?perPage&direction&fromPost&fromCreateAt&collapsedThreads` | Root + replies (PostList) |
| POST | `/posts/ids` | Fetch posts by id |
| GET | `/posts/{id}/info` | Permalink resolution (channel/team, whether joined) |
| POST | `/posts/{id}/pin`, `/posts/{id}/unpin` | Pin/unpin |
| GET | `/channels/{id}/pinned` | Pinned posts |
| POST | `/users/{uid}/posts/{pid}/set_unread` | Mark unread from this post |
| POST | `/teams/{tid}/posts/search` (or `/posts/search`) | `{terms, is_or_search, time_zone_offset, page, per_page, include_deleted_channels}` |
| POST | `/posts/{id}/actions/{action_id}` | Interactive message button/menu |

### 5.4 Reactions

| Method | Path | Purpose |
|---|---|---|
| POST | `/reactions` | `{user_id, post_id, emoji_name}` |
| DELETE | `/users/{uid}/posts/{pid}/reactions/{emoji_name}` | Remove |
| GET | `/posts/{id}/reactions` | List (usually unnecessary: included in `post.metadata.reactions`) |

### 5.5 Threads (CRT)

| Method | Path | Purpose |
|---|---|---|
| GET | `/users/{uid}/teams/{tid}/threads?per_page&before&after&since&extended&deleted&unread&totalsOnly&threadsOnly` | List threads + totals |
| GET | `/users/{uid}/teams/{tid}/threads/{thread_id}` | One thread |
| PUT | `/users/{uid}/teams/{tid}/threads/read` | Mark all read |
| PUT | `/users/{uid}/teams/{tid}/threads/{thread_id}/read/{timestamp}` | Mark read up to ts |
| POST | `/users/{uid}/teams/{tid}/threads/{thread_id}/set_unread/{post_id}` | Mark unread |
| PUT / DELETE | `/users/{uid}/teams/{tid}/threads/{thread_id}/following` | Follow / unfollow |

Response: `{total, total_unread_threads, total_unread_mentions, total_unread_urgent_mentions, threads: [ThreadResponse]}`
where `ThreadResponse = {id, reply_count, last_reply_at, last_viewed_at, participants[], post, unread_replies, unread_mentions, is_urgent, delete_at}` (`SM/thread.go`).

### 5.6 Files

Upload (`CS/actions/file_actions.ts`, `server/channels/api4/file.go`):

```http
POST /api/v4/files
Content-Type: multipart/form-data; boundary=…

--…  name="channel_id"      <channel id>          ← must come FIRST
--…  name="client_ids"      <your local id>       ← one per file, before the files
--…  name="files"; filename="a.png"  <bytes>
```

Field order matters: the server streams the body and needs `channel_id` before the file
parts. Response `201 {"file_infos": [FileInfo], "client_ids": [..]}`. Alternative
non-multipart form: `POST /files?channel_id=…&filename=…` with the raw bytes as the body.
Then send the returned ids in `file_ids` of `POST /posts`.

| Method | Path | Purpose |
|---|---|---|
| GET | `/files/{id}` | Download (`?download=1` forces attachment) |
| GET | `/files/{id}/thumbnail` | Thumbnail (images) |
| GET | `/files/{id}/preview` | Preview (images) |
| GET | `/files/{id}/info` | FileInfo |
| GET | `/files/{id}/link` | Public link (if enabled) |
| GET | `/posts/{id}/files/info` | FileInfos of a post (also in `post.metadata.files`) |
| POST | `/teams/{tid}/files/search` | File search |

`FileInfo` has `id, user_id, post_id, channel_id, create_at, name, extension, size, mime_type, width, height, has_preview_image, mini_preview` (base64 tiny JPEG placeholder).

### 5.7 Users, statuses, emoji, preferences, misc

| Method | Path | Purpose |
|---|---|---|
| POST | `/users/ids` | Profiles by ids (body: array) |
| POST | `/users/usernames` | Profiles by usernames |
| GET | `/users?in_channel=…&page&per_page` / `?in_team=…` | List users |
| GET | `/users/autocomplete?in_team&in_channel&name&limit` | @-mention autocomplete |
| GET | `/users/{id}/image?_=<last_picture_update>` | Avatar (cache-bust with `last_picture_update`) |
| POST | `/users/status/ids` | Statuses by ids (body: array) |
| GET / PUT | `/users/{id}/status` | Get / set status (`online`, `away`, `dnd`, `offline`, `ooo`) |
| PUT / DELETE | `/users/me/status/custom` | Custom status |
| POST | `/users/{id}/typing` | Typing (REST alternative to the WS action) |
| PUT | `/users/{id}/preferences` | Save preferences (array) |
| POST | `/users/{id}/preferences/delete` | Delete preferences (array) |
| GET | `/emoji?page&per_page&sort` | Custom emoji list |
| POST | `/emoji/names` | Custom emoji by names |
| GET | `/emoji/autocomplete?name=` / POST `/emoji/search` | Emoji search |
| GET | `/emoji/{id}/image` | Custom emoji image |
| GET | `{siteUrl}/static/emoji/{unified}.png` | System emoji image (served by the webapp bundle) |
| POST | `/commands/execute` | `{channel_id, team_id, root_id, command}` |
| GET | `/teams/{tid}/commands/autocomplete_suggestions?user_input=…` | Slash command autocomplete |
| GET / PUT / DELETE | `/users/{uid}/teams/{tid}/drafts`, `/drafts`, `/users/{uid}/channels/{cid}/drafts` | Synced drafts |
| POST | `/actions/dialogs/submit` | Interactive dialog submit |

---

## 6. Bootstrap sequence

What the webapp does from cold start to a rendered channel:

1. **`Root.componentDidMount` → `loadConfigAndMe()`** (`CS/components/root/actions/index.ts`)
   - In parallel: `GET /config/client?format=old`, `GET /license/client`.
   - If not logged in → login page.
   - In parallel: `GET /users/me`, `GET /users/me/preferences`, `GET /users/me/teams`,
     `GET /users/me/teams/members`. User/team member roles trigger `POST /roles/names`.
   - Then `GET /users/me/teams/unread?include_collapsed_threads=<crt>`.
   - (Redux-level equivalent: `MR/actions/users.ts` `loadMe()`.)
2. **Pick team/channel** — `redirectUserToDefaultTeam` (`CS/actions/global_actions.tsx`):
   last team from local storage (`user_prev_team:<uid>`), last channel
   (`user_team_prev_channel:<uid>:<tid>`), else `town-square`; navigate to
   `/{team}/channels/{name}`.
3. **WebSocket connect** — `LoggedIn` mounts and calls `initialize()` in
   `CS/actions/websocket_actions.ts` (§7).
4. **Team load** — `TeamController` (`CS/components/team_controller/`):
   - `GET /users/me/channels` (all teams) and
     `GET /users/{me}/channel_members?page=-1` (NDJSON) —
     `MR/actions/channels.ts` `fetchAllMyTeamsChannels` / `fetchAllMyChannelMembers`.
     (Per-team variant: `fetchChannelsAndMembers(teamId)`.)
   - `initializeTeam`: select team, start status polling
     (`POST /users/status/ids` for visible users), load groups if licensed.
5. **Sidebar** — `fetchMyCategories(teamId)` → `GET /users/me/teams/{tid}/channels/categories`;
   `loadProfilesForSidebar()` fetches profiles of DM/GM partners
   (`POST /users/ids`, plus `GET /users/group_channels` style calls for GMs).
6. **Channel entry** — `emitChannelClickEvent` / `PostList`
   (`CS/actions/views/channel.ts`, `CS/components/post_view/post_list/post_list.tsx`):
   - First visit: `GET /users/{uid}/channels/{cid}/posts/unread?limit_before=30&limit_after=30`
     (`loadUnreads`).
   - Already loaded: `GET /channels/{cid}/posts?since=<last sync time>` (`syncPostsInChannel`).
   - Permalink: `loadPostsAround`.
   - Then `POST /channels/members/me/view` (`markChannelAsRead`).
   - After posts arrive: fetch missing user profiles and statuses for authors/mentions
     (`batchFetchStatusesProfilesGroupsFromPosts`), custom emoji used in posts.
   - `GET /channels/{cid}/stats` for member count.
7. **Prefetch** — `CS/components/data_prefetch/` prefetches posts for unread channels.
8. **Threads view (CRT)** — `getThreadCounts` + `getThreadsForCurrentTeam`
   (`MR/actions/threads.ts`).

### Recommended startup for a Rust client

```
ping → config/client + license/client (parallel)
→ (login if no stored token)
→ users/me, preferences, teams, teams/members (parallel) → roles/names
→ users/me/channels + channel_members?page=-1 (parallel)
→ teams/unread
→ categories for the selected team
→ open WebSocket (send authentication_challenge, wait for hello)
→ profiles for DM/GM partners + statuses
→ open the last channel: posts/unread window → channels/members/me/view
```

Open the WebSocket **before** your channel/post fetches finish (or record the time before
fetching and do a `since=` sync after connecting) so no event falls between the REST
snapshot and the live stream.

---

## 7. WebSocket protocol

Client: `PC/websocket.ts` (`WebSocketClient`). App handling: `CS/actions/websocket_actions.ts`.
Server: `server/channels/api4/websocket.go`, `server/channels/app/platform/web_conn.go`,
`web_hub.go`, `websocket_router.go`; actions in `server/channels/wsapi/`. Event names:
`SM/websocket_message.go` and `PC/websocket_events.ts`; payload types:
`PC/websocket_messages.ts`. Go reference client: `SM/websocket_client.go`.

### 7.1 Connect

```
wss://host[/subpath]/api/v4/websocket?connection_id=<id or empty>&sequence_number=<next expected seq>&posted_ack=true
```

- Use `config.WebsocketURL` if set, otherwise derive from the site URL (`http`→`ws`,
  `https`→`wss`).
- `posted_ack=true` asks the server to want `posted_notify_ack` (push-notification
  acknowledgement); optional. `disconnect_err_code=<code>` reports why the previous socket
  closed; optional.

### 7.2 Authenticate

Either send `Authorization: Bearer <token>` on the upgrade request, or, after the socket
opens, send:

```json
{"seq": 1, "action": "authentication_challenge", "data": {"token": "<session token>"}}
```

Reply: `{"status": "OK", "seq_reply": 1}`, followed by the `hello` event. An
unauthenticated socket is closed after ~5 s.

Details that matter for a native client (`server/channels/api4/websocket.go`,
`server/channels/app/platform/websocket_router.go`, `web_hub.go`):

- **Resuming needs the header.** `connection_id`/`sequence_number` are only honoured when the
  upgrade request itself is authenticated; a socket authenticated by the challenge always
  gets a fresh connection id.
- A challenge on a socket that the header already authenticated is ignored (no reply).
- **An invalid token does not fail the upgrade.** The server answers `101`, and closes the
  socket when the challenge fails, without a close frame. A client can't tell this from a
  network drop; a connection that never gets a ping reply or event is "unconfirmed", and
  repeated ones are a reason to check the token with a REST call.
- `hello` is sent on fresh connections and when a resume fails (new connection id), **not**
  on a successful resume: the replayed or new events simply continue the old sequence.

### 7.3 Framing

All frames are JSON text (binary msgpack frames are also accepted after auth; ignore).

- **Client → server request**: `{"action": "...", "seq": N, "data": {...}}`. Client `seq`
  starts at 1 and increments per request; max message size 8 KB.
- **Server → client response**: `{"status": "OK"|"FAIL", "seq_reply": N, "data": {...}?, "error": AppError?}`.
- **Server → client event**:

```json
{
  "event": "posted",
  "data": {"post": "{\"id\":\"…\",\"message\":\"hi\",…}", "channel_type": "O", "mentions": "[\"uid\"]", …},
  "broadcast": {"omit_users": null, "user_id": "", "channel_id": "…", "team_id": "", "connection_id": ""},
  "seq": 7
}
```

Distinguish responses from events by the presence of `seq_reply` vs `event`.

### 7.4 Sequence numbers and resumption

- Event `seq` is per connection and starts at **0 with `hello`**.
- The client keeps `expected = 0`. For every event: if `seq != expected`, close the
  connection (code 4001) and reconnect; else `expected = seq + 1` (`PC/websocket.ts` ~L395).
- `hello.data = {server_version, connection_id, server_hostname}`. Store `connection_id`.
- On reconnect pass the stored `connection_id` and `sequence_number=expected`:
  - If the server still has the missed events, it **replays them**, continuing the old
    sequence. A disconnected connection stays in the hub, inactive, and keeps queueing events
    in its send queue (**256**, `sendQueueSize`). The **128**-event dead queue
    (`deadQueueSize`) holds frames already written to the dropped socket. Resume therefore
    works until the send queue overflows (the hub then drops the connection) or the reaper
    removes inactive connections (`inactiveConnReaperInterval`, 5 min) (`web_conn.go`,
    `web_hub.go`). Checked against 12.2: 152 events while offline resumed; 300 did not.
  - Otherwise it starts a new connection and sends `hello` with a **new** `connection_id`.
    The client must then **reset `expected` to 0 and run a full resync** (the
    `missedEventCallback` in `PC/websocket.ts` → `reconnect()` in
    `CS/actions/websocket_actions.ts`).

**Full resync** (what the webapp does in `reconnect()`):

- refetch my channels + channel members (all teams), sidebar categories
- current channel: `GET /channels/{id}/posts?since=<newest known create/update time>`
  (or a full load if never loaded); other loaded channels are marked stale
- team unreads (`/users/me/teams/unread`)
- threads: `getCountsAndThreadsSince` with the newest known `last_reply_at`
- users modified while offline (`checkForModifiedUsers`)
- resend `presence`; refetch client config if the server restarted (`server_version` changed)

### 7.5 Keepalive and reconnect

- Client sends `{"action": "ping", "seq": N}` every **30 s**; reply data
  `{text: "pong", version, server_time}`. If no pong before the next ping, close with code
  **4000** and reconnect.
- The server sends protocol-level pings every 60 s and drops the connection after ~100 s
  without a pong (most Rust WS libraries answer pings automatically).
- Reconnect backoff (`PC/websocket.ts`): 3 s while `failCount <= 7`; afterwards
  `3 s × failCount²`, capped at **5 min**; plus random jitter up to **2 s**.

### 7.6 Client actions

| Action | Data | Purpose |
|---|---|---|
| `authentication_challenge` | `{token}` | Authenticate |
| `ping` | – | Keepalive |
| `user_typing` | `{channel_id, parent_id}` | Typing indicator (throttle; see §15) |
| `presence` | `{channel_id}` or `{team_id}` or `{thread_channel_id, is_thread_view: true}` | Tell the server what you're viewing (affects notifications/push) |
| `user_update_active_status` | `{user_is_active, manual}` | Away/online activity |
| `get_statuses` | – | All statuses |
| `get_statuses_by_ids` | `{user_ids: [...]}` | Statuses for ids |
| `posted_notify_ack` | `{post_id, status, reason, …}` | Acknowledge a `posted` event that had `should_ack` |

### 7.7 Important events

Fields marked **(str)** are JSON-encoded strings that need a second `serde_json::from_str`.
The channel of an event is usually in `broadcast.channel_id`; team in `broadcast.team_id`.

| Event | Data | What the webapp does |
|---|---|---|
| `hello` | `server_version, connection_id, server_hostname` | Store connection id; detect server upgrade |
| `posted` | `post` (str), `channel_type`, `channel_name`, `channel_display_name`, `sender_name`, `team_id`, `set_online`, `mentions` (str: array of user ids), `followers` (str), `should_ack`, `image`, `otherFile` | Insert/replace pending post; update unread & mention counts; fetch missing profiles/root post; desktop notification (§13); mark read if viewing |
| `post_edited` | `post` (str) | Replace post |
| `post_deleted` | `post` (str), `delete_by?` | Mark deleted (render "(message deleted)"), drop its replies |
| `ephemeral_message` | `post` (str) | Show a local-only post (never persisted) |
| `post_unread` | `msg_count, msg_count_root, mention_count, mention_count_root, urgent_mention_count, last_viewed_at, post_id` | Set membership counters (manual "mark unread") |
| `reaction_added` / `reaction_removed` | `reaction` (str) | Update reactions |
| `post_acknowledgement_added` / `_removed` | `acknowledgement` (str) | Update acks (priority messages) |
| `typing` | `user_id, parent_id` | Show typing indicator for `broadcast.channel_id` |
| `status_change` | `user_id, status` | Update status. **Sent only to that user** (own status set on another device, or auto-away), not to others: `BroadcastStatus` in `platform/status.go` |
| `multiple_channels_viewed` | `channel_times: {channel_id: ms}` | Mark those channels read (other device/tab viewed them) |
| `channel_created`, `channel_updated` (`channel` str), `channel_deleted`, `channel_restored`, `channel_converted` | – | Update channel |
| `channel_member_updated` | `channelMember` (str) | Update my membership (notify props, roles, counts) |
| `user_added` / `user_removed` | `user_id`, `team_id` / `channel_id`, `remover_id` | Join/leave channel; if it's me, add/remove channel |
| `direct_added` / `group_added` | `creator_id` / `teammate_ids` (str) | New DM/GM: fetch channel + member |
| `added_to_team`, `leave_team`, `update_team`, `delete_team`, `restore_team` | team ids/team (str) | Team list changes |
| `new_user`, `user_updated` (`user` object, **not** encoded), `user_role_updated`, `user_activation_status_change` | – | Profile updates |
| `preference_changed` / `preferences_changed` / `preferences_deleted` | `preference`/`preferences` (str) | Update preferences (theme, CRT on/off, DM visibility, …) |
| `thread_updated` | `thread` (str), `previous_unread_mentions`, `previous_unread_replies` | Update thread + counts |
| `thread_read_changed` | `thread_id, timestamp, unread_mentions, unread_replies, channel_id, …` (or empty = all read) | Update thread read state |
| `thread_follow_changed` | `thread_id, state, reply_count` | Follow state |
| `sidebar_category_created` / `_updated` / `_deleted` / `_order_updated` | category ids / categories (str) / order | Sidebar. Every preference change also sends `sidebar_category_updated` with **no data and no `team_id`** (`app/preference.go`). The webapp ignores the payload: it refetches the team's categories when `broadcast.team_id` is set and does nothing otherwise |
| `emoji_added` | `emoji` (str) | New custom emoji |
| `role_updated`, `memberrole_updated`, `channel_scheme_updated` | role (str) | Refresh permissions |
| `config_changed`, `license_changed` | `config` / `license` (plain objects) | Replace client config/license |
| `open_dialog` | `dialog` (str) | Interactive dialog (§17) |
| `draft_created` / `draft_updated` / `draft_deleted` | `draft` (str) | Synced drafts |
| `scheduled_post_*`, `channel_bookmark_*`, `custom_profile_attributes_*`, `post_translation_updated`, `burn_on_read_*`, … | – | Newer features; safe to ignore initially |
| `plugin_*`, `received_group*`, `group_member_*`, `cloud_*`, `hosted_customer_*`, `job_updated`, … | – | Mostly ignorable for an MVP |

Treat unknown event names as no-ops — the list grows every release.

---

## 8. Data model and client state

### 8.1 Core entities (JSON field names)

- **Post** (`SM/post.go`, `PT/posts.ts`): `id, create_at, update_at, edit_at, delete_at,
  is_pinned, user_id, channel_id, root_id, original_id, message, type, props, hashtags,
  file_ids, pending_post_id, has_reactions, remote_id, reply_count, last_reply_at,
  participants, is_following, metadata`.
  `root_id == ""` → root post; otherwise a reply. `metadata` = `{embeds, emojis, files,
  images, reactions, priority, acknowledgements, …}`.
- **Channel** (`SM/channel.go`): `id, create_at, update_at, delete_at, team_id, type,
  display_name, name, header, purpose, last_post_at, total_msg_count, total_msg_count_root,
  last_root_post_at, creator_id, scheme_id, props, group_constrained, shared, …`.
  `type`: `O` public, `P` private, `D` direct, `G` group. DMs/GMs have `team_id == ""`.
- **ChannelMember** (`SM/channel_member.go`): `channel_id, user_id, roles, last_viewed_at,
  msg_count, msg_count_root, mention_count, mention_count_root, urgent_mention_count,
  notify_props, last_update_at, scheme_guest, scheme_user, scheme_admin, explicit_roles`.
  `notify_props`: `{desktop, push, email, mark_unread, ignore_channel_mentions,
  desktop_threads, push_threads, channel_auto_follow_threads, …}` (string values;
  `"default"` → use the user's setting).
- **User** (`SM/user.go`): `id, username, first_name, last_name, nickname, email, position,
  roles, locale, timezone, notify_props, props, last_picture_update, delete_at, is_bot,
  remote_id, …`. `notify_props`: `{desktop, desktop_sound, desktop_threads, email, push,
  mention_keys, first_name, channel, comments, …}`.
- **Team / TeamMember** (`SM/team.go`, `SM/team_member.go`): team `{id, name, display_name,
  type, …}`; member `{team_id, user_id, roles, delete_at, scheme_*}`. `TeamUnread`:
  `{team_id, msg_count, mention_count, msg_count_root, mention_count_root, thread_count,
  thread_mention_count, thread_urgent_mention_count}`.
- **Preference** (`SM/preference.go`): `{user_id, category, name, value}`. The webapp keys
  them as `"{category}--{name}"`.
- **Reaction**: `{user_id, post_id, emoji_name, create_at}`.
- **Status**: `{user_id, status, manual, last_activity_at, dnd_end_time}`.
- **ChannelCategory** (`PT/channel_categories.ts`): `{id, user_id, team_id, type, display_name,
  sorting, channel_ids, muted, collapsed}`.

### 8.2 Useful preferences

| Category | Name | Meaning |
|---|---|---|
| `display_settings` | `collapsed_reply_threads` | `on`/`off` — CRT (see §10.3) |
| `display_settings` | `name_format` | `username` / `nickname_full_name` / `full_name` |
| `display_settings` | `use_military_time` | 24h clock |
| `display_settings` | `message_display` | `clean` / `compact` |
| `display_settings` | `collapse_previews` | Collapse image/link previews by default |
| `display_settings` | `colorize_usernames` | Compact mode username colors |
| `display_settings` | `channel_display_mode` | `full` / `centered` |
| `direct_channel_show` | `<user_id>` | `true`/`false` — DM shown/closed in sidebar |
| `group_channel_show` | `<channel_id>` | GM shown/closed |
| `sidebar_settings` | `limit_visible_dms_gms` | Max DMs/GMs in sidebar (default 20; max 40) |
| `sidebar_settings` | `show_unread_section` | Group unreads at top |
| `favorite_channel` | `<channel_id>` | Legacy favorites (now categories) |
| `flagged_post` | `<post_id>` | Saved/flagged messages |
| `theme` | `<team_id>` or `""` | JSON theme colors |
| `notifications` | various | Email interval, etc. |
| `advanced_settings` | `send_on_ctrl_enter`, `code_block_ctrl_enter`, `join_leave` | Composer and join/leave visibility |
| `channel_approximate_view_time` / `channel_open_time` | `<channel_id>` | Used for DM auto-close ordering |

### 8.3 How the webapp stores it

The redux store (`PT/store.ts` `GlobalState`) is a normalized cache; a Rust client can use
the same layout:

```
entities.general        config (map<String,String>), license, serverVersion
entities.users          currentUserId, profiles{id→User}, statuses{id→String},
                        profilesInChannel{cid→Set}, profilesInTeam{tid→Set}, dndEndTimes
entities.teams          currentTeamId, teams{id→Team}, myMembers{tid→TeamMember (+unread counts)}
entities.channels       currentChannelId, channels{id→Channel}, channelsInTeam{tid→Set} ('' = DMs/GMs),
                        myMembers{cid→ChannelMember}, messageCounts{cid→{total, root}},
                        stats{cid→Stats}, manuallyUnread{cid→bool}
entities.posts          posts{id→Post}, postsInChannel{cid→[PostOrderBlock]},
                        postsInThread{root_id→[ids]}, reactions{post_id→{user-emoji→Reaction}},
                        pendingPostIds, openGraph{post_id→{url→OG}}
entities.threads        threads{id→Thread}, threadsInTeam{tid→[ids]}, unreadThreadsInTeam,
                        counts{tid→{total, total_unread_threads, total_unread_mentions, …}}
entities.preferences    myPreferences{"cat--name"→Preference}
entities.channelCategories  byId{id→Category}, orderByTeam{tid→[ids]}
entities.roles          roles{name→Role}
entities.emojis         customEmoji{id→Emoji}, nonExistentEmoji (negative cache)
websocket               connected, lastConnectAt, lastDisconnectAt, connectionId
```

Data flow: action → `Client4` call → `RECEIVED_*` action → reducer normalizes →
memoized selectors derive UI state. WebSocket events dispatch the same `RECEIVED_*` actions.
In Rust: one `Store` struct mutated by `apply(Event)`; UI reads derived views.

---

## 9. Posts and the timeline

### 9.1 PostList

Every post-fetching endpoint returns `PostList` (`SM/post_list.go`):

```json
{"order": ["newest_id", "…", "oldest_id"], "posts": {"id": Post, …},
 "next_post_id": "", "prev_post_id": "…", "has_next": false, "first_inaccessible_post_time": 0}
```

- `order` is newest-first. `posts` may contain extra posts not in `order` (e.g. root posts
  of replies, included so you can render "replying to…").
- `next_post_id == ""` → this page reaches the newest post; `prev_post_id == ""` → reached
  the oldest post.
- `first_inaccessible_post_time > 0` → older history is hidden by a license limit.

### 9.2 Ordered blocks

`MR/reducers/entities/posts.ts` stores each channel's timeline as a list of non-overlapping
**`PostOrderBlock { order: Vec<id> (newest first), recent: bool, oldest: bool }`**.

- Loading latest → new block with `recent = (next_post_id == "")`,
  `oldest = (prev_post_id == "")`.
- Loading before/after an anchor → block including the anchor so it merges with the
  existing one.
- `since` sync → add/update posts in the `recent` block, re-sort.
- New post via WebSocket → prepended to the `recent` block only (ignored if the channel was
  never loaded; replies skipped under CRT).
- After every change, `mergePostBlocks` sorts blocks and merges overlapping ones;
  `mergePostOrder` de-duplicates and sorts by `create_at` desc.
- `comparePosts` (`MR/utils/post_utils.ts`): pending/failed posts first, then `create_at`
  desc.

Page sizes: `POST_CHUNK_SIZE = 60`; unread window `limit_before = limit_after = 30`.
A simpler Rust design: per channel, a `BTreeMap<(create_at, id), PostId>` plus a list of
known-contiguous ranges and flags "reached newest/oldest".

### 9.3 Sending a post (optimistic)

`MR/actions/posts.ts` `createPost`:

1. Build `pending_post_id = format!("{user_id}:{now_ms}")`.
2. Insert locally a post with `id = pending_post_id`, `create_at = now`, `pending_post_id`.
3. `POST /posts` with `create_at: 0`, `pending_post_id`, `file_ids`, `root_id`, `props`.
4. The server response **and** the `posted` WebSocket event both carry the same
   `pending_post_id` — replace the pending post with the real one (whichever arrives first;
   ignore the duplicate).
5. On failure: mark `failed = true`, offer "Retry" (re-send with the same
   `pending_post_id`) and "Delete". Some errors drop the post instead (root deleted,
   read-only Town Square, plugin rejection `plugin.message_will_be_posted.dismiss_post`).

Max message length: client config `MaxPostSize` (computed by the server from the DB column
size; typically 16383 runes).

### 9.4 Edits, deletes, reactions, threads

- Edited posts have `edit_at > 0` → show "(edited)". Edit window may be limited by
  `PostEditTimeLimit` (config, seconds; `-1` = unlimited).
- Deleted posts (`delete_at > 0` or `post_deleted` event) render as "(message deleted)";
  remove their replies from the thread view.
- Reactions are in `post.metadata.reactions` (when `has_reactions`) and updated via events.
- Threads: `GET /posts/{root_id}/thread` returns root + replies. Without CRT, replies are
  shown inline in the channel with a "Commented on …" header.

### 9.5 Display-time transformations

`MR/utils/post_list.ts` (`makePreparePostIdsForPostList`, `makeFilterPostsAndAddSeparators`,
`makeCombineUserActivityPosts`) inserts virtual items:

- `date-<ts>` — date separators, computed in the **user's timezone**
  (`user.timezone`: `{useAutomaticTimezone, automaticTimezone, manualTimezone}`).
- `start-of-new-messages-<lastViewedAt>` — the "New Messages" line, before the first post
  with `create_at > last_viewed_at` that isn't your own (webhook posts count). Use the
  `last_viewed_at` captured **when you entered the channel**, not the updated one.
- `user-activity-<ids…>` — consecutive join/leave/add/remove system posts combined
  (max 100) into one "A, B and 3 others joined the channel" item.

Also: hide join/leave posts if the `advanced_settings`/`join_leave` preference is `false`
(`shouldFilterJoinLeavePost`; never hides ones about you), and "consecutive posts by the same
user within a few minutes" collapse the header (`areConsecutivePostsBySameUser`).

---

## 10. Unreads, mentions and Collapsed Reply Threads

### 10.1 Per-channel unread

`MR/utils/channel_utils.ts` `calculateUnreadCount`:

```rust
let (messages, mentions) = if crt_enabled {
    (channel.total_msg_count_root - member.msg_count_root, member.mention_count_root)
} else {
    (channel.total_msg_count - member.msg_count, member.mention_count)
};
let has_urgent = member.urgent_mention_count > 0;
let muted = member.notify_props.mark_unread == "mention";
let show_unread = mentions > 0 || (!muted && messages > 0);
```

### 10.2 Aggregates and live updates

- Team badges: `getTeamsUnreadStatuses` sums channel unreads per team (excluding DMs/GMs,
  muted and archived channels), plus thread counts when CRT is on. At startup
  `/users/me/teams/unread` gives initial totals for non-current teams.
- Global badge: `getUnreadStatus` — DMs/GMs count everywhere.
- Incoming `posted` for another channel → `channel.total_msg_count += 1` (and `_root` if
  root), `channel.last_post_at = post.create_at`; if `mentions` contains your id →
  `member.mention_count += 1` (and `_root`, `urgent_mention_count` if
  `metadata.priority.priority == "urgent"`). (`actionsToMarkChannelAsUnread` in
  `MR/actions/channels.ts`.)
- Incoming `posted` for the channel you're viewing (and the window is focused) → mark read
  (`POST /channels/members/me/view`), set `member.msg_count = channel.total_msg_count`,
  mention counts to 0, `last_viewed_at = now`.
- `multiple_channels_viewed` from another device → same local reset for those channels.
- Manual "Mark as unread" → `POST /users/{uid}/posts/{pid}/set_unread`; server replies
  with the counters and also sends `post_unread`. Keep a `manually_unread` flag so the
  channel isn't immediately re-marked read while you're in it.

### 10.3 Collapsed Reply Threads (CRT)

Enablement (`MR/selectors/entities/preferences.ts`):

- Config `CollapsedThreads`: `disabled` | `default_off` | `default_on` | `always_on`.
- Allowed = config ≠ `disabled`.
- Enabled = `always_on`, OR allowed and preference
  `display_settings/collapsed_reply_threads == "on"` (default `on` for `default_on`,
  `off` for `default_off`).

When enabled:

- Channel timelines show only root posts with a "N replies" footer
  (`reply_count`, `participants`, `last_reply_at`); pass `collapsedThreads=true` (and
  `collapsedThreadsExtended=true` to get participant profiles) when fetching posts.
- Unread math uses the `_root` counters (§10.1); channel sort by "recent" uses
  `last_root_post_at`.
- A separate **Threads** view lists followed threads (`/users/{uid}/teams/{tid}/threads`)
  with per-thread `unread_replies`, `unread_mentions`, `is_urgent`. Team-level totals come
  from the same endpoint (`totalsOnly=true`) and are added to team badges.
- Opening a thread → `PUT …/threads/{id}/read/{now_ms}`.
- Keep in sync with `thread_updated`, `thread_read_changed`, `thread_follow_changed`.
- Desktop notifications for replies respect `notify_props.desktop_threads` and whether you
  follow the thread (`followers` field of `posted`).

---

## 11. Message rendering

### 11.1 Markdown

- Engine: a Mattermost fork of `marked` (`webapp/channels/package.json`), wrapped in
  `CS/utils/markdown/index.ts` (`format`, `formatWithRenderer`) with
  `{sanitize: true, gfm: true, tables: true, mangle: false}`. **Raw HTML is escaped, not
  rendered.**
- Custom renderer `CS/utils/markdown/renderer.tsx`:
  - fenced code → code block with language; `latex`/`tex` fences → LaTeX block
    (only if config `EnableLatex == "true"`).
  - inline LaTeX `$…$` only if `EnableLatex` and `EnableInlineLatex`.
  - images support size syntax: `![alt](url =WIDTHxHEIGHT)` (`parseImageDimensions`).
  - links: unsafe schemes dropped (`isUrlSafe`), missing scheme → `http://`; links to the
    same site URL are handled as in-app navigation (permalinks, channels).
  - autolink only schemes in `getAutolinkedUrlSchemes` (http, https, ftp, mailto, tel,
    mattermost + config `CustomUrlSchemes`).
  - task lists, tables, headings.
- Plain text for notifications/previews: `stripMarkdown()` (`remove_markdown.ts`).

Rust: `comrak` (GFM, tables, autolink, tasklists, strikethrough; disable raw HTML) gets you
close. Implement Mattermost extensions as a post-processing pass over text nodes (not inside
code spans/blocks).

### 11.2 Text formatting pipeline

`CS/utils/text_formatting.tsx` `formatText` → markdown → `doFormatText` replaces matches with
tokens, in this order:

1. `@username` mentions (`autolinkAtMentions`) — also `@channel`, `@all`, `@here`, `@group`.
   Username pattern allows `[a-z0-9.\-_]`; trailing `.`/`_`/`-` is tried with and without.
2. `~channel-name` mentions (`autolinkChannelMentions`) — resolved from
   **`post.props.channel_mentions`** (`{name: {display_name, team_name}}`, provided by the
   server) with fallback to known channels.
3. Email addresses.
4. `#hashtags` (≥3 chars; clicking searches).
5. Emoticons and `:shortcode:` emoji (`CS/utils/emoticons.tsx`;
   `EMOJI_PATTERN = /(?<!\w)(:([\w+-]+):)(?!\w)/`; text emoticons like `:)`, `<3`).
6. Search term highlighting (in search results).
7. Highlight **your mention keys** (§13.2).
8. Unicode emoji detection.

Token limit: 1000 tokens/message (`FORMAT_TOKEN_LIMIT`). Messages consisting only of emoji
(≤ some count) render large (`all-emoji` class).

### 11.3 Emoji

- System emoji: data generated from `emoji-datasource` (v6.1.1) into `CS/utils/emoji.json` /
  `emoji.ts` (`Emojis`, `EmojiIndicesByAlias`) by `webapp/channels/build/emoji/make_emojis.mjs`
  (+ `additional_shortnames.json`). Reuse that JSON in Rust (short names → unified code
  points → image file name). Images are served at `{siteUrl}/static/emoji/{unified}.png`,
  or render the Unicode char with a color emoji font.
- Skin tones: preference + `_tone` variants.
- Custom emoji: `GET /emoji…`, image at `/emoji/{id}/image`; posts include the custom emoji
  they use in `post.metadata.emojis`. Unknown names should be cached as non-existent to
  avoid refetch loops.
- Reactions use the emoji **name** (`+1`, `smile`, or custom name).

### 11.4 Embeds, attachments, files

`post.metadata` (`PT/posts.ts` `PostMetadata`) is computed by the server — prefer it over
fetching anything yourself:

- `embeds: [{type: "image"|"link"|"message_attachment"|"opengraph"|"permalink", url, data}]`
  - `opengraph` → link preview card (`data` = OpenGraph object: title, description, images).
  - `image` → inline image preview of a linked image.
  - `permalink` → quoted preview of another Mattermost post (`data` contains the post).
  - `message_attachment` → the post has Slack-style `props.attachments`.
  - YouTube links get a special player in the webapp.
- `images: {url: {width, height, format, frame_count}}` — dimensions for layout.
- `files: [FileInfo]`, `reactions`, `emojis`, `priority: {priority: "important"|"urgent", requested_ack}`,
  `acknowledgements`.
- External images may go through the image proxy (config `ImageProxyType`); use the URL as
  given in the post or `metadata.images`.
- Link previews can be disabled per post: `props.previewed_post` / the "remove preview"
  action sets `props`.

**Message attachments** (`PT/message_attachments.ts`, renderer
`CS/components/post_view/message_attachments/`): `props.attachments: [{fallback, color,
pretext, author_name, author_link, author_icon, title, title_link, text, fields: [{title,
value, short}], image_url, thumb_url, footer, footer_icon, actions: [{id, name, type:
"button"|"select", style, options, data_source, integration}]}]`. Text fields are markdown.

### 11.5 Other

- LaTeX: KaTeX in the webapp. Rust options: render to image via an external tool, or show
  source in a code block.
- Code highlighting: highlight.js with lazy language loading
  (`CS/utils/syntax_highlighting.tsx`). Rust: `syntect`.
- Display names in posts honour `props.override_username` / `props.override_icon_url`
  for webhooks/bots (if config `EnablePostUsernameOverride` / `EnablePostIconOverride`),
  with a "BOT" tag if `props.from_webhook == "true"` or the user `is_bot`.

---

## 12. System messages and post types

Constants: `MR/constants/posts.ts` (`PostTypes`). Any `type` starting with `system_` is a
system message. Common ones:

| Type | Props used |
|---|---|
| `system_join_channel`, `system_leave_channel` | `username` |
| `system_add_to_channel`, `system_remove_from_channel` | `username`, `addedUsername`/`removedUsername`, `userId`, `addedUserId` |
| `system_join_team`, `system_leave_team`, `system_add_to_team`, `system_remove_from_team` | same pattern |
| `system_header_change` | `username`, `old_header`, `new_header` |
| `system_displayname_change` | `old_displayname`, `new_displayname` |
| `system_purpose_change` | `old_purpose`, `new_purpose` |
| `system_channel_deleted`, `system_channel_restored` | `username` |
| `system_convert_channel` (to private), `system_change_chan_privacy` | `username` |
| `system_ephemeral` | ephemeral (bot/command responses) |
| `system_generic` | message is displayed as-is |
| `system_combined_user_activity` | virtual, created client-side |
| `me` | `/me` action messages (italic) |
| `reminder` | `/remind` notifications |
| `custom_*` | plugin post types |

Rendering: `CS/components/post_markdown/system_message_helpers.tsx` (`systemMessageRenderers`,
`renderSystemMessage`) builds **localized** text from props. `post.message` contains a
server-generated English fallback — showing it is acceptable for an MVP.

Unknown `custom_*` types (plugin-rendered in the webapp) → fall back to rendering
`post.message` as markdown plus any attachments.

Ephemeral posts (`ephemeral_message` event) exist only on the client; drop them on reload.

---

## 13. Notifications

### 13.1 When to notify

`CS/actions/notification_actions.tsx` `sendDesktopNotification(post, msgProps)` is called
from the `posted` handler. `shouldSkipNotification` decides, in order:

1. Skip own posts (unless `props.from_webhook`).
2. Skip system messages, except "you were added to the channel".
3. Skip if `isNotificationSuppressed` (e.g. certain props).
4. Skip if you're not a member of the channel.
5. `props.force_notification` → **notify**, skip remaining checks.
6. Skip if the channel is muted (`notify_props.mark_unread == "mention"`).
7. Skip if your status is `dnd` or `ooo`.
8. Notify level = channel `notify_props.desktop`, or the user's `notify_props.desktop` if
   `"default"` (a GM at `default` with user `mention` behaves like `all`):
   - `none` → skip
   - `mention` → skip unless your user id is in `data.mentions` (for GMs, the client also
     matches your mention keys against the text)
   - `all` → notify; for CRT replies only if you follow the thread
     (`notify_props.desktop_threads`, `data.followers`)
9. Skip if the window is focused and you're looking at that channel/thread.

Title = channel display name (or sender for DMs); body = `@sender: <stripped markdown>`
(`getNotificationTitle`, `getNotificationBody`; honours `override_username`); files/images
→ "sent an image/attachment". Sound: user `notify_props.desktop_sound` /
`desktop_notification_sound`.

### 13.2 Mention keys

`MR/selectors/entities/users.ts` `getCurrentUserMentionKeys`:

- `@username` (always)
- `notify_props.mention_keys` — comma-separated custom keywords
- first name (case-sensitive) if `notify_props.first_name == "true"`
- `@channel`, `@all`, `@here` if `notify_props.channel == "true"` and not ignored for the
  channel (`ignore_channel_mentions`)
- group mentions (`@group-name`) for groups you belong to (`getAllUserMentionKeys`)

Used both for highlighting and for client-side mention detection in GMs.

Server-side push notifications (mobile) are not involved for a desktop client; the server
decides those.

---

## 14. Channel sidebar

- Categories: `GET /users/{uid}/teams/{tid}/channels/categories` →
  `{categories: [...], order: [ids]}`. Types: `favorites`, `channels`, `direct_messages`,
  `custom`, `managed`. Each has `channel_ids`, `sorting`, `muted`, `collapsed`.
- Mutations: `PUT …/categories` (bulk), `PUT …/categories/{id}`, `POST …/categories`,
  `PUT …/categories/order`, `DELETE …/categories/{id}`. Events `sidebar_category_*`.
- `MR/actions/channel_categories.ts`, `MR/selectors/entities/channel_categories.ts`.

Sorting (`makeSortChannels`):

| `sorting` | Order |
|---|---|
| `manual` | `channel_ids` order as stored |
| `alpha` / `""` (default) | display name (locale-aware, DMs by teammate display name) |
| `recent` | `max(last_post_at` (or `last_root_post_at` under CRT)`, create_at)` desc |

For `alpha` and `""`, muted channels go last (`makeCompareChannels`); names compare by the
user's locale with numbers as numbers (`localeCompare(…, {numeric: true})`), GM names are the
other members' display names, sorted.

Filters, in this order (`makeFilterAndSortChannelsForCategory`); the current channel passes
every one:

- **Archived** channels are hidden (`makeFilterArchivedChannels`).
- **DM/GM visibility is opt-in** (`makeFilterManuallyClosedDMs`): a DM shows only if the
  preference `direct_channel_show/<other_user_id>` exists and isn't `"false"` (a GM:
  `group_channel_show/<channel_id>`), or it's unread. A DM without the preference is hidden
  once read. The webapp keeps them visible by **saving `"true"` itself** for every unread DM/GM
  that lacks it, after loading the channels (`loadProfilesForDM`/`loadProfilesForGM` in
  `CS/actions/user_actions.ts`) and when a new DM arrives (`loadNewDMIfNeeded`). The self-DM's
  key is the empty name (`channel.name` with my id stripped), so it is usually hidden.
- **Autoclose**, in the `direct_messages` category only (`makeFilterAutoclosedDMs`): DMs with a
  deactivated user are hidden unless opened after the deactivation (`last_viewed_at`, or the
  `channel_approximate_view_time`/`channel_open_time` preferences); then at most
  `sidebar_settings/limit_visible_dms_gms` (**default 40**, `getVisibleDmGmLimit`) stay, or
  the number of unread ones if larger, ranked current → unread → last viewed.

**Unreads group** (`sidebar_settings/show_unread_section`, default from config
`ExperimentalGroupUnreadChannels == "default_on"`): unread channels of the team and DMs/GMs at
the top, out of their categories, sorted muted last → with mentions → most recent
(`sortUnreadChannels`). A collapsed category still shows its unread and current channels.

Display names:

- DM channel `name` = `"{id1}__{id2}"` (ids sorted); find the other id
  (`getUserIdFromChannelName`). Display = teammate's display name.
- GM display = comma-separated display names of the other members, sorted.
- Teammate display setting (`getTeammateNameDisplaySetting`): preference
  `display_settings/name_format`, unless config `LockTeammateNameDisplay` (licensed), else
  config `TeammateNameDisplay`, else `username`.
- `displayUsername` (`MR/utils/user_utils.ts`): `nickname_full_name` → nickname, else full
  name, else username; `full_name` → full name else username; `username` → username.

---

## 15. Typing, presence, statuses, drafts

- **Typing out**: send WS `user_typing {channel_id, parent_id}` at most once per
  `TimeBetweenUserTypingUpdatesMilliseconds` (config, ~5 s) while the user types.
- **Typing in**: `typing` event → show "X is typing…" for `(channel_id, parent_id)`; clear
  after the same interval or when that user's post arrives
  (`CS/components/msg_typing/actions.ts`). Only shown if config `EnableUserTypingMessages`.
- **Presence/activity**: send `presence` with the viewed channel/thread; send
  `user_update_active_status {user_is_active: true/false, manual: false}` based on
  OS idle state — the server sets `away` after inactivity.
- **Statuses**: `status_change` only arrives for your own status (see §7); other users'
  statuses must be polled with `POST /users/status/ids` (the webapp polls visible users every
  ~60 s). Statuses:
  `online`, `away`, `dnd` (+`dnd_end_time`), `offline`, `ooo`. Custom status lives in
  `user.props.customStatus` (JSON string: emoji, text, expires_at).
- **Drafts**: kept locally per channel and per thread (`CS/actions/views/drafts.ts`). Synced to
  the server only if config `AllowSyncedDrafts == "true"` and the user hasn't disabled it:
  `PUT /drafts`, `GET /users/{uid}/teams/{tid}/drafts`, `DELETE /users/{uid}/channels/{cid}/drafts`
  (+`?root_id`), events `draft_*`.

---

## 16. Permissions

- Role names come from `user.roles`, `team_member.roles`, `channel_member.roles`
  (space-separated) plus scheme flags (`scheme_user`, `scheme_admin`, `scheme_guest`) which map to
  the team/channel scheme's roles.
- Fetch definitions with `POST /roles/names` → `[{name, permissions: [..], scheme_managed, …}]`.
  Refresh on `role_updated` / `memberrole_updated` / `channel_scheme_updated`.
- `haveIChannelPermission(team, channel, perm)` = system roles ∪ team roles ∪ channel roles
  contain `perm` (`MR/selectors/entities/roles.ts`, `roles_helpers.ts`).
- Common permissions: `create_post`, `edit_post`, `edit_others_posts`, `delete_post`,
  `delete_others_posts`, `add_reaction`, `remove_reaction`, `upload_file`,
  `use_channel_mentions`, `create_public_channel`, `create_private_channel`,
  `manage_public_channel_properties`, `read_channel`, `create_direct_channel`.
- Edit/delete UI also checks authorship and `PostEditTimeLimit` (`canEditPost`,
  `canDeletePost` in `CS/utils/post_utils.ts`). The server enforces everything anyway; client
  checks are only for hiding UI.
- Archived channels (`delete_at > 0`) are read-only; Town Square may be read-only by policy.

---

## 17. Slash commands, integrations and plugins

### Slash commands (`CS/actions/command.ts` `executeCommand`)

- Client-only commands in the webapp: `/search`, `/shortcuts`, `/leave` (with confirmation
  for private channels), `/settings`, `/marketplace`, `/collapse`, `/expand`, `/logout`.
  Reimplement the ones you need.
- Everything else → `POST /commands/execute {channel_id, team_id, root_id, command}`.
  The response may contain `goto_location` (URL to open/navigate), `trigger_id`
  (for a follow-up dialog), and the command's text response arrives as a post or
  ephemeral post via WebSocket.
- Autocomplete: `GET /teams/{tid}/commands/autocomplete_suggestions?user_input=…`.
- Apps framework commands (`AppCommandParser`) are optional.

### Interactive messages

- Buttons/menus in `props.attachments[].actions` → `POST /posts/{post_id}/actions/{action_id}`
  (`{selected_option, cookie}` for menus; `doPostActionWithCookie` in `MR/actions/posts.ts`).
  The response may update the post (via `post_edited`) or open a dialog.
- Dialogs: `open_dialog` event with `dialog` (str) containing `trigger_id`, `url`, `dialog:
  {title, elements: [...], submit_label, notify_on_cancel, state}`. Only open if `trigger_id`
  matches one you recently received. Submit via `POST /actions/dialogs/submit`
  (`CS/actions/integration_actions.tsx`, `CS/utils/interactive_dialog.ts`).

### Webapp plugins

Webapp plugins (`CS/plugins/index.ts`, `CS/plugins/registry.ts`, ~70 `register*` APIs) are JS
bundles that inject React components. **A native client cannot run them** — ignore
`/plugins/webapp` and `plugin_*` events. Server-side plugin behaviour (slash commands, bots,
posts with attachments, interactive dialogs) still works through the APIs above.
Plugin custom post types → fallback rendering (§12).

---

## 18. Routing and deep links

Webapp URLs you'll find in messages and must parse (`CS/components/root/root.tsx`,
`CS/components/channel_layout/center_channel/center_channel.tsx`):

| URL | Meaning |
|---|---|
| `/{team}/channels/{channel_name}` | Channel |
| `/{team}/messages/@{username}` | DM with user |
| `/{team}/messages/{user_id}` / `/{team}/messages/{gm_name}` | DM by id / GM |
| `/{team}/pl/{post_id}` | Permalink (resolve with `GET /posts/{id}/info`, join channel if allowed, then load posts before/after it — `getPostsAround` — and its thread) |
| `/_redirect/pl/{post_id}` | Permalink without known team |
| `/{team}/threads/{thread_id?}` | Threads view (CRT) |
| `/{team}/drafts` | Drafts view |

Identifier resolution (`getPathFromIdentifier` in
`CS/components/channel_layout/channel_identifier_router/actions.ts`):

- under `channels/`: known name; 26 chars → try as channel id; 40 chars → GM name;
  `id__id` (54 chars) → DM name.
- under `messages/`: `@username`, email, 26-char user id → DM; 40 chars → GM.

Patterns: team name `[a-z0-9\-_]+`, ids `[a-z0-9]{26}` (`CS/utils/path.ts`). Only treat a link
as internal if it starts with the configured site URL.

---

## 19. Local persistence

What the webapp persists:

- Drafts (IndexedDB via redux-persist + localForage, `CS/store/index.ts`,
  `CS/reducers/storage.ts`).
- localStorage (`CS/stores/local_storage_store.ts`): last team (`user_prev_team:<uid>`),
  last channel per team (`user_team_prev_channel:<uid>:<tid>`), penultimate channel, recent
  emoji (`recent_emojis:<uid>`).
- Everything in `entities.*` is **refetched** on every load.

Suggestions for Rust:

- Store the session token in the OS keychain (`keyring` crate), along with the server URL
  and user id.
- Optionally cache entities (users, channels, posts) in SQLite to render instantly, then
  reconcile: channels/members refetch, `posts?since=` per cached channel, ETag on profiles.
- Persist: last team/channel, drafts (unless synced), recent emoji, window state.
- On logout: delete token, wipe cache.

---

## 20. Suggested Rust architecture

### Crates

| Concern | Crate |
|---|---|
| Async runtime | `tokio` |
| HTTP | `reqwest` (rustls, multipart, stream) |
| WebSocket | `tokio-tungstenite` |
| JSON | `serde`, `serde_json` (with `#[serde(default)]`), `serde_with` for string-encoded fields |
| Markdown | `comrak` (GFM) or `pulldown-cmark` |
| Syntax highlighting | `syntect` |
| Emoji data | embed `CS/utils/emoji.json`, or `emojis` crate |
| Secrets | `keyring` |
| Cache | `rusqlite` / `sqlx` (SQLite) |
| Notifications | `notify-rust` (Linux/Windows), `mac-notification-sys` |
| Time zones | `chrono` + `chrono-tz` |
| UI | your choice: `iced`, `egui`, `slint`, `gtk4-rs`, `tauri` (webview) |

### Modules

```
api/      Typed REST client: base URL, Bearer auth, AppError parsing, NDJSON, 429 backoff,
          one fn per endpoint (mirror SM/client4.go names)
ws/       Connection task: connect, auth challenge, hello, seq tracking, ping/pong,
          resume (connection_id + sequence_number), backoff; emits typed Event enum
          (double-parse the JSON-string fields here)
model/    serde structs (Post, Channel, ChannelMember, User, Team, Thread, Preference, …)
store/    Normalized state (see §8.3) + apply(Event) + derived views (unreads, sidebar)
sync/     Bootstrap, channel loading (unread window, before/after paging, since-sync),
          resync after missed events, mark-read, optimistic posting
render/   Markdown → your UI's rich text: mentions, ~channels, emoji, hashtags, code,
          attachments, embeds, system message strings
notify/   shouldSkipNotification rules, mention keys, OS notifications, badge counts
ui/       Views: team switcher, sidebar, channel, thread, search, settings
```

### Milestones

1. Login (password + MFA) with Bearer token; bootstrap; team/channel list; read-only
   channel view with paging.
2. WebSocket with seq/resume; live posts, edits, deletes, reactions; posting with pending
   ids; mark read; unread badges.
3. Markdown + mentions + emoji + system messages + combined activity posts; profiles/avatars.
4. Sidebar categories, DM/GM names, DM auto-close; statuses; typing.
5. Threads (non-CRT inline replies, then CRT Threads view).
6. Files (upload/download/previews), link previews, attachments, search.
7. Desktop notifications, slash commands, interactive messages/dialogs.
8. SSO, local cache, settings (notify props, preferences), i18n.

### Testing

Run a local server (`make run-server` in `server/`, or the official Docker image) and test
against it. The Go client tests in `server/channels/api4/*_test.go` show expected request/
response behaviour for each endpoint.

---

## 21. Key source files index

| File | Why it matters |
|---|---|
| `PC/client4.ts` | Every REST endpoint, headers, login, response parsing |
| `PC/websocket.ts` | WS connect, auth, seq check, resume, ping, backoff |
| `PC/websocket_events.ts`, `PC/websocket_messages.ts` | Event names and payload types |
| `PT/*.ts` (`posts.ts`, `channels.ts`, `users.ts`, `threads.ts`, `config.ts`, `store.ts`) | Data shapes and client state |
| `SM/client4.go`, `SM/websocket_client.go` | Go reference clients |
| `SM/websocket_message.go` | Canonical event list + envelope |
| `SM/post.go`, `SM/channel.go`, `SM/channel_member.go`, `SM/user.go`, `SM/thread.go`, `SM/preference.go` | JSON wire format |
| `api/v4/source/*.yaml` | OpenAPI spec (auth/WS docs in `introduction.yaml`) |
| `server/channels/app/platform/web_conn.go`, `web_hub.go` | Server-side WS semantics (dead queue, hello, timeouts) |
| `CS/components/root/actions/index.ts` | `loadConfigAndMe` bootstrap |
| `CS/actions/websocket_actions.ts` | WS init, reconnect resync, per-event handling |
| `CS/actions/new_post.ts`, `CS/actions/post_actions.ts` | Handling incoming posts, unread updates |
| `CS/actions/views/channel.ts` | Channel switching, post loading (unreads, before/after, since) |
| `MR/actions/posts.ts` | createPost (pending ids), edits, reactions, post actions |
| `MR/actions/channels.ts` | Channel fetching, mark read/unread |
| `MR/actions/threads.ts` | CRT thread API |
| `MR/reducers/entities/posts.ts` | Post order blocks and merging |
| `MR/utils/post_list.ts` | Date separators, new-messages line, combined activity posts |
| `MR/utils/channel_utils.ts` | `calculateUnreadCount`, mute, DM names |
| `MR/selectors/entities/channels.ts` | Team/global unread aggregation |
| `MR/selectors/entities/channel_categories.ts` | Sidebar sorting and DM auto-close |
| `MR/selectors/entities/preferences.ts` | CRT enablement, teammate name display |
| `MR/selectors/entities/users.ts` | Mention keys |
| `MR/selectors/entities/roles.ts` | Permission checks |
| `CS/actions/notification_actions.tsx` | Desktop notification rules |
| `CS/utils/markdown/index.ts`, `renderer.tsx` | Markdown options and renderer |
| `CS/utils/text_formatting.tsx` | Mentions/emoji/hashtag pipeline |
| `CS/utils/emoticons.tsx`, `CS/utils/emoji.json` | Emoji patterns and data |
| `CS/components/post_markdown/system_message_helpers.tsx` | System message rendering |
| `CS/actions/command.ts` | Slash command handling |
| `CS/components/channel_layout/channel_identifier_router/actions.ts` | URL identifier resolution |
