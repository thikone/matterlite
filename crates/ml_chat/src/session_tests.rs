// SPDX-License-Identifier: AGPL-3.0-only

use std::cell::RefCell;
use std::collections::HashMap;
use std::time::{Duration, Instant};

use mm_model::{SessionToken, UserId};
use serde_json::json;
use warpui::App;
use warpui_extras::secure_storage::{self, SecureStorage};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::*;

#[test]
fn token_keys_are_valid_file_names() {
    let key = token_key("0123456789abcdef");
    assert_eq!(key, "token-0123456789abcdef");
    assert!(!key.contains([':', '/', '\\']));
}

#[test]
fn account_ids_come_from_directory_names() {
    assert_eq!(
        account_id_of(Path::new("state/servers/0123456789abcdef")),
        "0123456789abcdef"
    );
}

#[derive(Default)]
struct MemoryStorage(RefCell<HashMap<String, String>>);

impl SecureStorage for MemoryStorage {
    fn write_value(&self, key: &str, value: &str) -> Result<(), secure_storage::Error> {
        self.0.borrow_mut().insert(key.to_owned(), value.to_owned());
        Ok(())
    }

    fn read_value(&self, key: &str) -> Result<String, secure_storage::Error> {
        self.0
            .borrow()
            .get(key)
            .cloned()
            .ok_or(secure_storage::Error::NotFound)
    }

    fn remove_value(&self, key: &str) -> Result<(), secure_storage::Error> {
        self.0.borrow_mut().remove(key);
        Ok(())
    }
}

fn stored_token(app: &App, key: &str) -> Option<String> {
    app.read(|ctx| {
        use warpui_extras::secure_storage::AppContextExt as _;
        ctx.secure_storage().read_value(key).ok()
    })
}

fn alice() -> User {
    serde_json::from_value(json!({
        "id": "u".repeat(26),
        "username": "alice",
        "first_name": "Alice",
    }))
    .unwrap()
}

/// Waits (letting the app's tasks run) until `done` holds, for at most 10 s.
async fn until(app: &App, done: impl Fn(&Session) -> bool) {
    let start = Instant::now();
    while !app.read(|ctx| done(Session::as_ref(ctx))) {
        assert!(start.elapsed() < Duration::from_secs(10), "timed out");
        warpui::r#async::Timer::after(Duration::from_millis(20)).await;
    }
}

#[test]
fn an_expired_session_returns_to_the_login_with_the_account_filled_in() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let server = runtime.block_on(async {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v4/users/me"))
            .respond_with(ResponseTemplate::new(401).set_body_json(json!({
                "id": "api.context.session_expired.app_error",
                "message": "Invalid or expired session, please login again.",
                "status_code": 401,
            })))
            .mount(&server)
            .await;
        server
    });
    let servers_dir = tempfile::tempdir().unwrap();
    let url = ServerUrl::parse(&server.uri()).unwrap();
    let key = token_key(&mm_cache::account_id(
        url.as_str(),
        &UserId::new("u".repeat(26)),
    ));

    App::test((), |mut app| async move {
        app.add_singleton_model(|_| -> secure_storage::Model {
            Box::new(MemoryStorage::default())
        });
        let dir = servers_dir.path().to_owned();
        let session = app.add_singleton_model(move |_| Session::new(dir));
        session.update(&mut app, |session, ctx| {
            session.sign_in(
                url.clone(),
                LoginResponse {
                    user: alice(),
                    token: SessionToken::new("secret-token"),
                },
                ctx,
            )
        });
        assert!(app.read(|ctx| Session::as_ref(ctx).active().is_some()));
        assert_eq!(stored_token(&app, &key).as_deref(), Some("secret-token"));

        until(&app, |session| session.active().is_none()).await;
        app.read(|ctx| match Session::as_ref(ctx).state() {
            SessionState::SignedOut(signed_out) => {
                assert!(
                    signed_out
                        .notice
                        .as_deref()
                        .unwrap()
                        .contains("session ended")
                );
                assert_eq!(signed_out.server.as_deref(), Some(url.as_str()));
                assert_eq!(signed_out.login_id.as_deref(), Some("alice"));
            }
            SessionState::Active(_) => unreachable!(),
        });
        assert_eq!(stored_token(&app, &key), None);
    });
    drop(server);
}
