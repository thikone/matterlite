// SPDX-License-Identifier: AGPL-3.0-only

use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::*;

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

fn auth() -> Auth {
    Auth::new("MatterLite tests".to_owned())
}

fn config(pairs: &[(&str, &str)]) -> ClientConfig {
    ClientConfig(
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect(),
    )
}

#[test]
fn placeholder_names_the_enabled_methods() {
    let both = SignInOptions::from_config(&config(&[
        ("EnableSignInWithEmail", "true"),
        ("EnableSignInWithUsername", "true"),
    ]));
    assert_eq!(both.login_placeholder(), "Email or Username");
    assert!(both.password());
    let three = SignInOptions::from_config(&config(&[
        ("EnableSignInWithEmail", "true"),
        ("EnableSignInWithUsername", "true"),
        ("EnableLdap", "true"),
        ("LdapLoginFieldName", "Corp ID"),
    ]));
    assert_eq!(three.login_placeholder(), "Email, Username or Corp ID");
    let sso_only = SignInOptions::from_config(&config(&[("EnableSignUpWithGitLab", "true")]));
    assert!(!sso_only.password());
    assert_eq!(sso_only.sso, ["GitLab"]);
}

#[test]
fn needs_a_url() {
    let error = runtime().block_on(auth().probe("  ")).unwrap_err();
    assert_eq!(error.message, "Enter the server URL.");
    let error = runtime().block_on(auth().probe("ftp://host")).unwrap_err();
    assert!(error.message.contains("http://"), "{error}");
}

#[test]
fn without_a_scheme_https_comes_first() {
    let urls = candidates("chat.example.com/mm/").unwrap();
    let urls: Vec<&str> = urls.iter().map(ServerUrl::as_str).collect();
    assert_eq!(
        urls,
        ["https://chat.example.com/mm", "http://chat.example.com/mm"]
    );
    assert_eq!(candidates("http://host").unwrap().len(), 1);
}

#[test]
fn probe_reads_the_sign_in_options() {
    let runtime = runtime();
    let auth = auth();
    let server = runtime.block_on(async {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v4/system/ping"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("X-Version-Id", "12.2.0.12.2.0.abc.false")
                    .set_body_json(json!({"status": "OK"})),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v4/config/client"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "SiteName": "Test Chat",
                "EnableSignInWithEmail": "false",
                "EnableSignInWithUsername": "true",
                "EnableSaml": "true",
            })))
            .mount(&server)
            .await;
        server
    });
    // Typed without a scheme, the https attempt fails to connect and http answers.
    let typed = server.uri().trim_start_matches("http://").to_owned();
    let info = runtime.block_on(auth.probe(&typed)).unwrap();
    assert_eq!(info.url.as_str(), server.uri());
    assert_eq!(info.site_name, "Test Chat");
    assert_eq!(
        info.version.map(|version| version.to_string()).as_deref(),
        Some("12.2.0 (build 12.2.0)")
    );
    assert_eq!(info.sign_in.login_placeholder(), "Username");
    assert_eq!(info.sign_in.sso, ["SAML"]);
}

#[test]
fn probe_of_something_else() {
    let runtime = runtime();
    let auth = auth();
    let server = runtime.block_on(MockServer::start());
    let error = runtime.block_on(auth.probe(&server.uri())).unwrap_err();
    assert_eq!(
        error.message,
        "There is no Mattermost server at this address."
    );
}

#[test]
fn login_errors_are_worded_for_people() {
    let runtime = runtime();
    let auth = auth();
    let server = runtime.block_on(async {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v4/users/login"))
            .respond_with(ResponseTemplate::new(401).set_body_json(json!({
                "id": "mfa.validate_token.authenticate.app_error",
                "message": "Invalid MFA token.",
                "status_code": 401,
            })))
            .mount(&server)
            .await;
        server
    });
    let url = ServerUrl::parse(&server.uri()).unwrap();
    let request = LoginRequest {
        login_id: "frank".to_owned(),
        password: "secret".to_owned(),
        ..LoginRequest::default()
    };
    let error = runtime.block_on(auth.login(url, request)).unwrap_err();
    assert!(error.needs_mfa);
    assert_eq!(
        error.message,
        "Enter the MFA code from your authenticator app."
    );

    let invalid = ApiError::Unauthorized(mm_model::AppError {
        id: "api.user.login.invalid_credentials_email_username".to_owned(),
        ..Default::default()
    });
    assert_eq!(describe(&invalid), "The login or password is wrong.");
    let unknown = ApiError::App(mm_model::AppError {
        id: "some.other.app_error".to_owned(),
        message: "Something else.".to_owned(),
        ..Default::default()
    });
    assert_eq!(describe(&unknown), "Something else.");
}
