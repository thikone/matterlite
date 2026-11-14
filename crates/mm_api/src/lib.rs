// SPDX-License-Identifier: AGPL-3.0-only

//! The Mattermost REST client (`/api/v4`, WebApp.md §3–§5).
//!
//! One [`Client`] per server. Requests carry `X-Requested-With: XMLHttpRequest` and, once
//! logged in, `Authorization: Bearer <token>`; cookies are never stored or sent, so CSRF
//! doesn't apply. Endpoint method names follow the Go client in
//! `server/public/model/client4.go`.

mod client;
mod error;
mod server_url;

pub use client::{
    Client, ClientOptions, LoginRequest, LoginResponse, NewPost, PostsQuery, ThreadsQuery,
    default_user_agent,
};
pub use error::ApiError;
pub use server_url::{ServerUrl, ServerUrlError};
