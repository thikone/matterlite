// SPDX-License-Identifier: AGPL-3.0-only

use std::fmt;

use url::Url;

/// A server's site URL, e.g. `https://chat.example.com` or `https://example.com/mattermost`.
/// Kept without a trailing slash, query or fragment.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ServerUrl(Url);

#[derive(Debug, thiserror::Error)]
pub enum ServerUrlError {
    #[error("not a URL: {0}")]
    Parse(#[from] url::ParseError),
    #[error("the server URL must start with http:// or https://")]
    Scheme,
}

impl ServerUrl {
    /// Parses what a user typed. Without a scheme, `https://` is assumed.
    pub fn parse(input: &str) -> Result<Self, ServerUrlError> {
        let input = input.trim();
        let mut url = if input.contains("://") {
            Url::parse(input)?
        } else {
            Url::parse(&format!("https://{input}"))?
        };
        if !matches!(url.scheme(), "http" | "https") {
            return Err(ServerUrlError::Scheme);
        }
        url.set_query(None);
        url.set_fragment(None);
        let path = url.path().trim_end_matches('/').to_owned();
        url.set_path(&path);
        Ok(Self(url))
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str().trim_end_matches('/')
    }

    /// `<site>/api/v4/<segments...>`, each segment percent-encoded.
    pub fn api_url(&self, segments: &[&str]) -> Url {
        api_url(self.0.clone(), segments)
    }

    /// The WebSocket endpoint: `ws(s)://<base>/api/v4/websocket`, where the base is the
    /// config's `WebsocketURL` when the admin set one and the site URL otherwise.
    pub fn websocket_url(&self, websocket_base: Option<&str>) -> Result<Url, ServerUrlError> {
        let base = match websocket_base {
            Some(base) => Url::parse(base.trim())?,
            None => self.0.clone(),
        };
        let scheme = match base.scheme() {
            "http" | "ws" => "ws",
            "https" | "wss" => "wss",
            _ => return Err(ServerUrlError::Scheme),
        };
        let mut url = api_url(base, &["websocket"]);
        url.set_scheme(scheme)
            .map_err(|()| ServerUrlError::Scheme)?;
        url.set_query(None);
        Ok(url)
    }
}

fn api_url(mut url: Url, segments: &[&str]) -> Url {
    url.path_segments_mut()
        .expect("http(s) and ws(s) URLs have a path")
        .pop_if_empty()
        .extend(["api", "v4"])
        .extend(segments);
    url
}

impl fmt::Display for ServerUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
#[path = "server_url_tests.rs"]
mod tests;
