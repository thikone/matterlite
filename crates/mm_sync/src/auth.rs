// SPDX-License-Identifier: AGPL-3.0-only

//! The requests made before an engine exists: checking a server and logging in
//! (MatterLite.md §10.2 `LoginView`, WebApp.md §3).
//!
//! [`Auth`] runs them on its own small Tokio runtime, so the UI can await the returned
//! futures on any executor.

use std::future::Future;

use mm_api::{ApiError, Client, ClientOptions, LoginRequest, LoginResponse, ServerUrl};
use mm_model::error::ids as error_ids;
use mm_model::{ClientConfig, ServerVersion};

/// What the login screen needs to know about a server.
#[derive(Clone, Debug, PartialEq)]
pub struct ServerInfo {
    /// The URL that answered (`http://` when only that worked for a URL typed without a
    /// scheme).
    pub url: ServerUrl,
    /// `SiteName`, "Mattermost" when unset.
    pub site_name: String,
    pub version: Option<ServerVersion>,
    pub sign_in: SignInOptions,
}

/// The sign-in methods the server offers (from the unauthenticated client config).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SignInOptions {
    pub email: bool,
    pub username: bool,
    pub ldap: bool,
    /// What the server calls the LDAP login field (`LdapLoginFieldName`), if set.
    pub ldap_field_name: Option<String>,
    /// SSO providers, which MatterLite supports from M7 on.
    pub sso: Vec<&'static str>,
}

impl SignInOptions {
    pub fn from_config(config: &ClientConfig) -> Self {
        let sso = [
            ("EnableSaml", "SAML"),
            ("EnableSignUpWithGitLab", "GitLab"),
            ("EnableSignUpWithGoogle", "Google"),
            ("EnableSignUpWithOffice365", "Office 365"),
            ("EnableSignUpWithOpenId", "OpenID"),
        ]
        .into_iter()
        .filter(|(key, _)| config.bool(key))
        .map(|(_, name)| name)
        .collect();
        Self {
            email: config.bool("EnableSignInWithEmail"),
            username: config.bool("EnableSignInWithUsername"),
            ldap: config.bool("EnableLdap"),
            ldap_field_name: config
                .get("LdapLoginFieldName")
                .filter(|name| !name.is_empty())
                .map(str::to_owned),
            sso,
        }
    }

    /// Whether a login id and password can be used at all.
    pub fn password(&self) -> bool {
        self.email || self.username || self.ldap
    }

    /// The login field's placeholder, worded as in the webapp (`login.tsx`,
    /// `getInputPlaceholder`): "Email or Username".
    pub fn login_placeholder(&self) -> String {
        let mut names = Vec::new();
        if self.email {
            names.push("Email".to_owned());
        }
        if self.username {
            names.push("Username".to_owned());
        }
        if self.ldap {
            names.push(
                self.ldap_field_name
                    .clone()
                    .unwrap_or_else(|| "AD/LDAP Username".to_owned()),
            );
        }
        match names.split_last() {
            None => String::new(),
            Some((last, [])) => last.clone(),
            Some((last, rest)) => format!("{} or {last}", rest.join(", ")),
        }
    }
}

/// A failed server check or login, worded for the login screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthError {
    pub message: String,
    /// The login must be repeated with an MFA code.
    pub needs_mfa: bool,
}

impl AuthError {
    fn from_api(error: &ApiError) -> Self {
        Self {
            message: describe(error),
            needs_mfa: error.needs_mfa_code(),
        }
    }
}

impl std::fmt::Display for AuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// Runs server checks and logins on a dedicated runtime.
pub struct Auth {
    runtime: tokio::runtime::Runtime,
    user_agent: String,
}

impl Auth {
    pub fn new(user_agent: String) -> Self {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .thread_name("auth")
            .enable_all()
            .build()
            .expect("tokio runtime");
        Self {
            runtime,
            user_agent,
        }
    }

    /// Checks that a Mattermost server answers at what the user typed and reads its sign-in
    /// options. Without a scheme, `https://` is tried first and then `http://`.
    pub fn probe(
        &self,
        input: &str,
    ) -> impl Future<Output = Result<ServerInfo, AuthError>> + Send + use<> {
        let candidates = candidates(input);
        let options = self.options();
        let task = self.runtime.spawn(async move {
            let candidates = candidates?;
            let mut first_error = None;
            for url in candidates {
                match probe(url, options.clone()).await {
                    Ok(info) => return Ok(info),
                    // Another scheme only helps when nothing answered.
                    Err(error) if matches!(error, ApiError::Http(_)) => {
                        first_error.get_or_insert(error);
                    }
                    Err(error) => return Err(probe_error(&error)),
                }
            }
            Err(probe_error(&first_error.expect("at least one candidate")))
        });
        async move { task.await.expect("probe task") }
    }

    /// `POST /users/login`. When the error has `needs_mfa`, ask for the code and repeat.
    pub fn login(
        &self,
        server: ServerUrl,
        request: LoginRequest,
    ) -> impl Future<Output = Result<LoginResponse, AuthError>> + Send + use<> {
        let options = self.options();
        let task = self.runtime.spawn(async move {
            let client =
                Client::new(server, options).map_err(|error| AuthError::from_api(&error))?;
            client
                .login(&request)
                .await
                .map_err(|error| AuthError::from_api(&error))
        });
        async move { task.await.expect("login task") }
    }

    fn options(&self) -> ClientOptions {
        ClientOptions {
            user_agent: self.user_agent.clone(),
            ..ClientOptions::default()
        }
    }
}

fn candidates(input: &str) -> Result<Vec<ServerUrl>, AuthError> {
    let input = input.trim();
    if input.is_empty() {
        return Err(AuthError {
            message: "Enter the server URL.".to_owned(),
            needs_mfa: false,
        });
    }
    let parse = |text: &str| {
        ServerUrl::parse(text).map_err(|error| AuthError {
            message: format!("That isn't a server URL: {error}."),
            needs_mfa: false,
        })
    };
    if input.contains("://") {
        Ok(vec![parse(input)?])
    } else {
        Ok(vec![
            parse(&format!("https://{input}"))?,
            parse(&format!("http://{input}"))?,
        ])
    }
}

async fn probe(url: ServerUrl, options: ClientOptions) -> Result<ServerInfo, ApiError> {
    let client = Client::new(url.clone(), options)?;
    client.ping().await?;
    let config = client.client_config().await?;
    Ok(ServerInfo {
        url,
        site_name: config
            .get("SiteName")
            .filter(|name| !name.is_empty())
            .unwrap_or("Mattermost")
            .to_owned(),
        version: client.version(),
        sign_in: SignInOptions::from_config(&config),
    })
}

fn probe_error(error: &ApiError) -> AuthError {
    let message = match error {
        ApiError::Http(_) => format!("Can't reach the server: {}", describe(error)),
        ApiError::Status { status: 404, .. } | ApiError::Decode { .. } => {
            "There is no Mattermost server at this address.".to_owned()
        }
        _ => describe(error),
    };
    AuthError {
        message,
        needs_mfa: false,
    }
}

/// A user-facing description of a failed request.
pub fn describe(error: &ApiError) -> String {
    if let Some(app) = error.app_error() {
        let id = app.id.as_str();
        return if id.starts_with(error_ids::INVALID_CREDENTIALS_PREFIX) {
            "The login or password is wrong.".to_owned()
        } else if id == error_ids::USER_INACTIVE {
            "This account is deactivated.".to_owned()
        } else if id == error_ids::MFA_BAD_CODE {
            "The MFA code is wrong.".to_owned()
        } else if id == error_ids::MFA_REQUIRED_FOR_LOGIN {
            "Enter the MFA code from your authenticator app.".to_owned()
        } else if id == error_ids::MFA_SETUP_REQUIRED {
            "This server requires multi-factor authentication. Set it up in the web app first."
                .to_owned()
        } else if id == error_ids::TOO_MANY_LOGIN_ATTEMPTS {
            "Too many failed attempts: the account is locked. An administrator can unlock it."
                .to_owned()
        } else if !app.message.is_empty() {
            app.message.clone()
        } else {
            app.id.clone()
        };
    }
    match error {
        ApiError::RateLimited { .. } => {
            "The server is limiting requests. Try again in a moment.".to_owned()
        }
        ApiError::Http(error) => {
            // reqwest's own text is "error sending request for url (…)"; the cause that
            // matters (DNS, refused, certificate) is the last one in the chain.
            let mut cause: &dyn std::error::Error = error;
            while let Some(source) = cause.source() {
                cause = source;
            }
            cause.to_string()
        }
        other => other.to_string(),
    }
}

#[cfg(test)]
#[path = "auth_tests.rs"]
mod tests;
