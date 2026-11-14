// SPDX-License-Identifier: AGPL-3.0-only

use std::time::Duration;

use mm_model::AppError;

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    /// The server refused the request with an `AppError` body. Branch on its `id`.
    #[error("{0}")]
    App(AppError),
    /// `401`: the token is missing, expired or revoked (or, for a login, the credentials or
    /// MFA code are wrong). Outside of login the user must log in again.
    #[error("unauthorized: {0}")]
    Unauthorized(AppError),
    /// `429`. The body is plain text, so there is no `AppError`.
    #[error("rate limited by the server")]
    RateLimited { retry_after: Option<Duration> },
    /// An error status without an `AppError` body (for example from a proxy).
    #[error("HTTP {status}: {body}")]
    Status { status: u16, body: String },
    #[error("request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("cannot decode the response of {path}: {source}")]
    Decode {
        path: String,
        #[source]
        source: serde_json::Error,
    },
    /// The login succeeded but the response had no `Token` header.
    #[error("the login response has no session token")]
    MissingToken,
}

impl ApiError {
    pub fn app_error(&self) -> Option<&AppError> {
        match self {
            Self::App(error) | Self::Unauthorized(error) => Some(error),
            Self::RateLimited { .. }
            | Self::Status { .. }
            | Self::Http(_)
            | Self::Decode { .. }
            | Self::MissingToken => None,
        }
    }

    /// The login must be repeated with an MFA code.
    pub fn needs_mfa_code(&self) -> bool {
        self.app_error().is_some_and(AppError::needs_mfa_code)
    }

    /// On an authenticated call: the session ended (`api.context.session_expired.app_error`
    /// or another 401), so log in again.
    pub fn is_session_expired(&self) -> bool {
        matches!(self, Self::Unauthorized(_))
    }
}
