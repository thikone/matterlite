// SPDX-License-Identifier: AGPL-3.0-only

use serde::{Deserialize, Serialize};

/// The server's error body (`server/public/model/utils.go` `AppError`). Branch on `id`; the
/// `message` is localized.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(default)]
#[error("{message} ({id})")]
pub struct AppError {
    pub id: String,
    pub message: String,
    pub detailed_error: String,
    pub request_id: String,
    pub status_code: u16,
}

/// Error ids a client reacts to.
pub mod ids {
    /// Login needs an MFA code, or the code was missing.
    pub const MFA_REQUIRED_FOR_LOGIN: &str = "mfa.validate_token.authenticate.app_error";
    /// Login was sent with a wrong MFA code.
    pub const MFA_BAD_CODE: &str = "api.user.check_user_mfa.bad_code.app_error";
    /// MFA is enforced on the server but the user has not set it up.
    pub const MFA_SETUP_REQUIRED: &str = "api.context.mfa_required.app_error";
    /// The session expired or was revoked.
    pub const SESSION_EXPIRED: &str = "api.context.session_expired.app_error";
    /// Wrong login id or password (the id names the sign-in methods that are enabled, e.g.
    /// `..._email_username`), so compare with `starts_with`.
    pub const INVALID_CREDENTIALS_PREFIX: &str = "api.user.login.invalid_credentials";
    /// The account is deactivated.
    pub const USER_INACTIVE: &str = "api.user.login.inactive.app_error";
    /// `GET /users/me/channels` for a user in no channel (a 404, not an empty list).
    pub const CHANNELS_NOT_FOUND: &str = "app.channel.get_channels.not_found.app_error";
    /// No channel membership; also what `GET /users/{id}/channel_members` returns for a user in
    /// no channel.
    pub const CHANNEL_MEMBER_MISSING: &str = "app.channel.get_member.missing.app_error";
    /// A WebSocket request on a connection whose session was revoked or expired. The server
    /// keeps such sockets open but stops sending events.
    pub const WS_NOT_AUTHENTICATED: &str = "api.web_socket_router.not_authenticated.app_error";
}

impl AppError {
    /// Whether the login must be repeated with an MFA code (WebApp.md §3.3).
    pub fn needs_mfa_code(&self) -> bool {
        self.id == ids::MFA_REQUIRED_FOR_LOGIN || self.id == ids::MFA_BAD_CODE
    }
}
