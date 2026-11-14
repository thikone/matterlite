// SPDX-License-Identifier: AGPL-3.0-only

use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mm_model::error::ids;
use mm_model::{
    AppError, Channel, ChannelId, ChannelMember, ClientConfig, OrderedSidebarCategories, Post,
    PostId, PostList, Preference, Role, ServerVersion, SessionToken, Status, Team, TeamId,
    TeamMember, TeamUnread, ThreadList, User, UserId,
};
use reqwest::header::{HeaderMap, RETRY_AFTER};
use reqwest::{Method, RequestBuilder, Response, StatusCode};
use serde::de::DeserializeOwned;
use serde_json::json;

use crate::error::ApiError;
use crate::server_url::ServerUrl;

/// `MatterLite/<version> (<os>)`.
pub fn default_user_agent() -> String {
    format!(
        "MatterLite/{} ({})",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS
    )
}

#[derive(Clone, Debug)]
pub struct ClientOptions {
    pub user_agent: String,
    pub connect_timeout: Duration,
    /// For a whole request including the body.
    pub timeout: Duration,
}

impl Default for ClientOptions {
    fn default() -> Self {
        Self {
            user_agent: default_user_agent(),
            connect_timeout: Duration::from_secs(15),
            timeout: Duration::from_secs(60),
        }
    }
}

/// The REST client for one server. Cloning is cheap and shares the connection pool.
#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    server: ServerUrl,
    token: Option<SessionToken>,
    /// The last `X-Version-Id` response header (server version and build).
    server_version: Arc<Mutex<Option<String>>>,
}

impl fmt::Debug for Client {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Client")
            .field("server", &self.server)
            .field("logged_in", &self.token.is_some())
            .finish()
    }
}

/// `POST /users/login`. The server reads the body as a string map.
#[derive(Clone, Default)]
pub struct LoginRequest {
    /// Username or email.
    pub login_id: String,
    pub password: String,
    /// The MFA code; empty on the first attempt.
    pub mfa_code: String,
    /// Push notification device id; empty for MatterLite.
    pub device_id: String,
    /// Only try LDAP.
    pub ldap_only: bool,
}

impl fmt::Debug for LoginRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LoginRequest")
            .field("login_id", &self.login_id)
            .field("has_mfa_code", &!self.mfa_code.is_empty())
            .field("ldap_only", &self.ldap_only)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug)]
pub struct LoginResponse {
    pub user: User,
    pub token: SessionToken,
}

/// Query of `GET /channels/{id}/posts`. With nothing set, the newest page.
#[derive(Clone, Debug, Default)]
pub struct PostsQuery {
    pub page: Option<u32>,
    /// Default 60, at most 200.
    pub per_page: Option<u32>,
    /// Posts older than this one.
    pub before: Option<PostId>,
    /// Posts newer than this one.
    pub after: Option<PostId>,
    /// Everything created, edited or deleted since this time (ms).
    pub since: Option<i64>,
    /// Leave replies out of the channel timeline (CRT).
    pub collapsed_threads: bool,
    /// With CRT, include the participants of each thread.
    pub collapsed_threads_extended: bool,
}

impl PostsQuery {
    fn pairs(&self) -> Vec<(&'static str, String)> {
        let mut pairs = Vec::new();
        if let Some(page) = self.page {
            pairs.push(("page", page.to_string()));
        }
        if let Some(per_page) = self.per_page {
            pairs.push(("per_page", per_page.to_string()));
        }
        if let Some(before) = &self.before {
            pairs.push(("before", before.to_string()));
        }
        if let Some(after) = &self.after {
            pairs.push(("after", after.to_string()));
        }
        if let Some(since) = self.since {
            pairs.push(("since", since.to_string()));
        }
        if self.collapsed_threads {
            pairs.push(("collapsedThreads", "true".to_owned()));
        }
        if self.collapsed_threads_extended {
            pairs.push(("collapsedThreadsExtended", "true".to_owned()));
        }
        pairs
    }
}

impl Client {
    pub fn new(server: ServerUrl, options: ClientOptions) -> Result<Self, ApiError> {
        let http = reqwest::Client::builder()
            .user_agent(options.user_agent)
            .connect_timeout(options.connect_timeout)
            .timeout(options.timeout)
            .build()?;
        Ok(Self {
            http,
            server,
            token: None,
            server_version: Arc::default(),
        })
    }

    pub fn server(&self) -> &ServerUrl {
        &self.server
    }

    pub fn token(&self) -> Option<&SessionToken> {
        self.token.as_ref()
    }

    pub fn set_token(&mut self, token: Option<SessionToken>) {
        self.token = token;
    }

    pub fn with_token(mut self, token: SessionToken) -> Self {
        self.token = Some(token);
        self
    }

    /// The server version from the most recent response (`X-Version-Id`).
    pub fn server_version(&self) -> Option<String> {
        self.server_version
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// [`Client::server_version`], parsed.
    pub fn version(&self) -> Option<ServerVersion> {
        ServerVersion::parse(&self.server_version()?)
    }

    fn request(&self, method: Method, segments: &[&str]) -> RequestBuilder {
        let builder = self
            .http
            .request(method, self.server.api_url(segments))
            .header("X-Requested-With", "XMLHttpRequest");
        match &self.token {
            Some(token) => builder.bearer_auth(token.expose()),
            None => builder,
        }
    }

    fn get(&self, segments: &[&str]) -> RequestBuilder {
        self.request(Method::GET, segments)
    }

    fn post(&self, segments: &[&str]) -> RequestBuilder {
        self.request(Method::POST, segments)
    }

    async fn send(&self, builder: RequestBuilder) -> Result<Response, ApiError> {
        let response = builder.send().await?;
        if let Some(version) = response
            .headers()
            .get("X-Version-Id")
            .and_then(|value| value.to_str().ok())
        {
            *self
                .server_version
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(version.to_owned());
        }
        check_status(response).await
    }

    async fn json<T: DeserializeOwned>(&self, builder: RequestBuilder) -> Result<T, ApiError> {
        let response = self.send(builder).await?;
        let path = response.url().path().to_owned();
        let body = response.bytes().await?;
        serde_json::from_slice(&body).map_err(|source| ApiError::Decode { path, source })
    }

    async fn empty(&self, builder: RequestBuilder) -> Result<(), ApiError> {
        self.send(builder).await.map(drop)
    }

    // Bootstrap / system (WebApp.md §5.1).

    /// `GET /system/ping`; works without a token.
    pub async fn ping(&self) -> Result<(), ApiError> {
        self.empty(self.get(&["system", "ping"])).await
    }

    /// `GET /config/client?format=old`; works without a token (with fewer keys).
    pub async fn client_config(&self) -> Result<ClientConfig, ApiError> {
        self.json(self.get(&["config", "client"]).query(&[("format", "old")]))
            .await
    }

    /// `GET /license/client?format=old`.
    pub async fn client_license(&self) -> Result<ClientConfig, ApiError> {
        self.json(self.get(&["license", "client"]).query(&[("format", "old")]))
            .await
    }

    /// `POST /users/login`. On success the session token comes from the `Token` response
    /// header; the client doesn't keep it (call [`Client::set_token`]). When
    /// [`ApiError::needs_mfa_code`] is true, repeat the login with `mfa_code` set.
    pub async fn login(&self, request: &LoginRequest) -> Result<LoginResponse, ApiError> {
        let mut body = json!({
            "login_id": request.login_id,
            "password": request.password,
            "token": request.mfa_code,
            "device_id": request.device_id,
        });
        if request.ldap_only {
            body["ldap_only"] = "true".into();
        }
        let response = self
            .send(self.post(&["users", "login"]).json(&body))
            .await?;
        let token = response
            .headers()
            .get("Token")
            .and_then(|value| value.to_str().ok())
            .filter(|token| !token.is_empty())
            .map(SessionToken::new)
            .ok_or(ApiError::MissingToken)?;
        let path = response.url().path().to_owned();
        let body = response.bytes().await?;
        let user =
            serde_json::from_slice(&body).map_err(|source| ApiError::Decode { path, source })?;
        Ok(LoginResponse { user, token })
    }

    /// `POST /users/logout`: revokes the session.
    pub async fn logout(&self) -> Result<(), ApiError> {
        self.empty(self.post(&["users", "logout"])).await
    }

    /// `GET /users/me`.
    pub async fn me(&self) -> Result<User, ApiError> {
        self.json(self.get(&["users", "me"])).await
    }

    /// `GET /users/me/preferences`.
    pub async fn my_preferences(&self) -> Result<Vec<Preference>, ApiError> {
        self.json(self.get(&["users", "me", "preferences"])).await
    }

    /// `GET /users/me/teams`.
    pub async fn my_teams(&self) -> Result<Vec<Team>, ApiError> {
        self.json(self.get(&["users", "me", "teams"])).await
    }

    /// `GET /users/me/teams/members`.
    pub async fn my_team_members(&self) -> Result<Vec<TeamMember>, ApiError> {
        self.json(self.get(&["users", "me", "teams", "members"]))
            .await
    }

    /// `GET /users/me/teams/unread`; with CRT the thread counts are included.
    pub async fn my_team_unreads(
        &self,
        include_collapsed_threads: bool,
    ) -> Result<Vec<TeamUnread>, ApiError> {
        self.json(
            self.get(&["users", "me", "teams", "unread"])
                .query(&[("include_collapsed_threads", include_collapsed_threads)]),
        )
        .await
    }

    /// `POST /roles/names`: roles with their permissions.
    pub async fn roles_by_names(&self, names: &[&str]) -> Result<Vec<Role>, ApiError> {
        self.json(self.post(&["roles", "names"]).json(names)).await
    }

    // Channels (WebApp.md §5.2).

    /// `GET /users/me/channels`: my channels in all teams, DMs and GMs included.
    ///
    /// A user in no channel gets a 404 `app.channel.get_channels.not_found.app_error` instead of
    /// an empty list (the server streams the response and fails on the first, empty page), which
    /// this returns as an empty list.
    pub async fn my_channels(&self) -> Result<Vec<Channel>, ApiError> {
        match self.json(self.get(&["users", "me", "channels"])).await {
            Err(ApiError::App(error))
                if error.status_code == 404 && error.id == ids::CHANNELS_NOT_FOUND =>
            {
                Ok(Vec::new())
            }
            other => other,
        }
    }

    /// `GET /users/{user_id}/channel_members?page=-1`: all my channel memberships. The server
    /// streams them as NDJSON; a JSON array is accepted too. A user in no channel gets a 404
    /// `app.channel.get_member.missing.app_error`, returned as an empty list.
    pub async fn my_channel_members(
        &self,
        user_id: &UserId,
    ) -> Result<Vec<ChannelMember>, ApiError> {
        let response = match self
            .send(
                self.get(&["users", user_id.as_str(), "channel_members"])
                    .query(&[("page", "-1")]),
            )
            .await
        {
            Err(ApiError::App(error))
                if error.status_code == 404 && error.id == ids::CHANNEL_MEMBER_MISSING =>
            {
                return Ok(Vec::new());
            }
            other => other?,
        };
        let path = response.url().path().to_owned();
        let body = response.bytes().await?;
        parse_json_lines(&body).map_err(|source| ApiError::Decode { path, source })
    }

    /// `GET /users/{user_id}/teams/{team_id}/channels/categories`.
    pub async fn sidebar_categories(
        &self,
        user_id: &UserId,
        team_id: &TeamId,
    ) -> Result<OrderedSidebarCategories, ApiError> {
        self.json(self.get(&[
            "users",
            user_id.as_str(),
            "teams",
            team_id.as_str(),
            "channels",
            "categories",
        ]))
        .await
    }

    /// `POST /channels/members/me/view`: mark a channel read (on all devices). Without it,
    /// unread counts never clear.
    pub async fn view_channel(
        &self,
        channel_id: &ChannelId,
        previous_channel_id: Option<&ChannelId>,
    ) -> Result<(), ApiError> {
        let body = json!({
            "channel_id": channel_id,
            "prev_channel_id": previous_channel_id.map(ChannelId::as_str).unwrap_or_default(),
            "collapsed_threads_supported": true,
        });
        self.empty(
            self.post(&["channels", "members", "me", "view"])
                .json(&body),
        )
        .await
    }

    // Posts (WebApp.md §5.3).

    /// `GET /channels/{id}/posts`.
    pub async fn channel_posts(
        &self,
        channel_id: &ChannelId,
        query: &PostsQuery,
    ) -> Result<PostList, ApiError> {
        self.json(
            self.get(&["channels", channel_id.as_str(), "posts"])
                .query(&query.pairs()),
        )
        .await
    }

    /// `GET /users/{uid}/channels/{cid}/posts/unread`: a window around the first unread post.
    pub async fn posts_around_first_unread(
        &self,
        user_id: &UserId,
        channel_id: &ChannelId,
        limit_before: u32,
        limit_after: u32,
        collapsed_threads: bool,
    ) -> Result<PostList, ApiError> {
        self.json(
            self.get(&[
                "users",
                user_id.as_str(),
                "channels",
                channel_id.as_str(),
                "posts",
                "unread",
            ])
            .query(&[
                ("limit_before", limit_before.to_string()),
                ("limit_after", limit_after.to_string()),
                ("collapsedThreads", collapsed_threads.to_string()),
            ]),
        )
        .await
    }

    // Users (WebApp.md §5.7).

    /// `POST /users/ids`.
    pub async fn users_by_ids(&self, user_ids: &[UserId]) -> Result<Vec<User>, ApiError> {
        self.json(self.post(&["users", "ids"]).json(user_ids)).await
    }

    /// `POST /users/status/ids`.
    pub async fn statuses_by_ids(&self, user_ids: &[UserId]) -> Result<Vec<Status>, ApiError> {
        self.json(self.post(&["users", "status", "ids"]).json(user_ids))
            .await
    }

    // Single entities, for events that name something the client doesn't have yet.

    /// `GET /channels/{id}`.
    pub async fn channel(&self, channel_id: &ChannelId) -> Result<Channel, ApiError> {
        self.json(self.get(&["channels", channel_id.as_str()]))
            .await
    }

    /// `GET /channels/{id}/members/me`.
    pub async fn my_channel_member(
        &self,
        channel_id: &ChannelId,
    ) -> Result<ChannelMember, ApiError> {
        self.json(self.get(&["channels", channel_id.as_str(), "members", "me"]))
            .await
    }

    /// `GET /teams/{id}`.
    pub async fn team(&self, team_id: &TeamId) -> Result<Team, ApiError> {
        self.json(self.get(&["teams", team_id.as_str()])).await
    }

    /// `GET /teams/{id}/members/me`.
    pub async fn my_team_member(&self, team_id: &TeamId) -> Result<TeamMember, ApiError> {
        self.json(self.get(&["teams", team_id.as_str(), "members", "me"]))
            .await
    }

    // Writing.

    /// `POST /posts` (WebApp.md §9.3). `pending_post_id` lets the server recognise a retry
    /// for 30 s and comes back in the reply and the `posted` event.
    pub async fn create_post(&self, post: &NewPost) -> Result<Post, ApiError> {
        let body = json!({
            "channel_id": post.channel_id,
            "root_id": post.root_id,
            "message": post.message,
            "pending_post_id": post.pending_post_id,
            "file_ids": post.file_ids,
            "props": post.props,
            "create_at": 0,
        });
        self.json(self.post(&["posts"]).json(&body)).await
    }

    /// `PUT /users/{id}/preferences`: saves (creates or replaces) preferences.
    pub async fn save_preferences(
        &self,
        user_id: &UserId,
        preferences: &[Preference],
    ) -> Result<(), ApiError> {
        self.empty(
            self.request(Method::PUT, &["users", user_id.as_str(), "preferences"])
                .json(preferences),
        )
        .await
    }

    // Threads (CRT, WebApp.md §10.3).

    /// `GET /users/{uid}/teams/{tid}/threads`: followed threads, newest activity first.
    pub async fn team_threads(
        &self,
        user_id: &UserId,
        team_id: &TeamId,
        query: &ThreadsQuery,
    ) -> Result<ThreadList, ApiError> {
        self.json(
            self.get(&[
                "users",
                user_id.as_str(),
                "teams",
                team_id.as_str(),
                "threads",
            ])
            .query(&query.pairs()),
        )
        .await
    }

    /// `GET /posts/{id}/thread`: the root and all replies.
    pub async fn post_thread(&self, root_id: &PostId) -> Result<PostList, ApiError> {
        self.json(self.get(&["posts", root_id.as_str(), "thread"]).query(&[
            ("collapsedThreads", "true"),
            ("collapsedThreadsExtended", "true"),
        ]))
        .await
    }
}

/// A post to send ([`Client::create_post`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NewPost {
    pub channel_id: ChannelId,
    /// Empty for a root post.
    pub root_id: PostId,
    pub message: String,
    /// `"{user_id}:{now_ms}"`.
    pub pending_post_id: String,
    pub file_ids: Vec<mm_model::FileId>,
    pub props: serde_json::Map<String, serde_json::Value>,
}

/// Query of [`Client::team_threads`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ThreadsQuery {
    /// Threads with activity since (ms), for a resync.
    pub since: Option<i64>,
    /// Older than this thread (paging).
    pub before: Option<PostId>,
    pub per_page: Option<u32>,
    pub unread_only: bool,
    /// Counts only, no threads.
    pub totals_only: bool,
}

impl ThreadsQuery {
    fn pairs(&self) -> Vec<(&'static str, String)> {
        let mut pairs = vec![("extended", "true".to_owned())];
        if let Some(since) = self.since {
            pairs.push(("since", since.to_string()));
        }
        if let Some(before) = &self.before {
            pairs.push(("before", before.to_string()));
        }
        if let Some(per_page) = self.per_page {
            pairs.push(("per_page", per_page.to_string()));
        }
        if self.unread_only {
            pairs.push(("unread", "true".to_owned()));
        }
        if self.totals_only {
            pairs.push(("totalsOnly", "true".to_owned()));
            pairs.push(("threadsOnly", "false".to_owned()));
        }
        pairs
    }
}

async fn check_status(response: Response) -> Result<Response, ApiError> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    if status == StatusCode::TOO_MANY_REQUESTS {
        return Err(ApiError::RateLimited {
            retry_after: retry_after(response.headers()),
        });
    }
    let body = response.text().await.unwrap_or_default();
    // Streaming endpoints write `[` before they know whether the query fails.
    let error_json = body.strip_prefix('[').unwrap_or(&body);
    match serde_json::from_str::<AppError>(error_json) {
        Ok(mut error) if !error.id.is_empty() => {
            if error.status_code == 0 {
                error.status_code = status.as_u16();
            }
            if status == StatusCode::UNAUTHORIZED {
                Err(ApiError::Unauthorized(error))
            } else {
                Err(ApiError::App(error))
            }
        }
        _ => {
            log::debug!("HTTP {status} without an AppError body");
            Err(ApiError::Status {
                status: status.as_u16(),
                body: body.chars().take(200).collect(),
            })
        }
    }
}

/// `Retry-After` or `X-RateLimit-Reset`, both in seconds.
fn retry_after(headers: &HeaderMap) -> Option<Duration> {
    [RETRY_AFTER.as_str(), "X-RateLimit-Reset"]
        .into_iter()
        .find_map(|name| headers.get(name)?.to_str().ok()?.trim().parse::<u64>().ok())
        .map(Duration::from_secs)
}

/// Newline-delimited JSON values, or one JSON array.
fn parse_json_lines<T: DeserializeOwned>(body: &[u8]) -> serde_json::Result<Vec<T>> {
    let start = body.iter().position(|byte| !byte.is_ascii_whitespace());
    if start.is_some_and(|start| body[start] == b'[') {
        return serde_json::from_slice(body);
    }
    serde_json::Deserializer::from_slice(body)
        .into_iter()
        .collect()
}

#[cfg(test)]
#[path = "client_tests.rs"]
mod tests;
