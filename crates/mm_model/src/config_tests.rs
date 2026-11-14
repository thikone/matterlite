// SPDX-License-Identifier: AGPL-3.0-only

use super::*;

#[test]
fn values_of_any_scalar_type_become_strings() {
    let config: ClientConfig = serde_json::from_str(
        r#"{"EnableLatex": "true", "MaxPostSize": 16383, "ExperimentalX": true,
            "WebsocketURL": "", "Unset": null, "CollapsedThreads": "default_on"}"#,
    )
    .unwrap();
    assert!(config.bool("EnableLatex"));
    assert!(config.bool("ExperimentalX"));
    assert!(!config.bool("Missing"));
    assert_eq!(config.int("MaxPostSize"), Some(16383));
    assert_eq!(config.websocket_url(), None, "empty means not set");
    assert_eq!(config.get("Unset"), None);
    assert_eq!(config.collapsed_threads(), CollapsedThreads::DefaultOn);
}

#[test]
fn collapsed_threads_rules() {
    use CollapsedThreads::*;
    assert!(!Disabled.is_enabled(Some("on")));
    assert!(AlwaysOn.is_enabled(Some("off")));
    assert!(DefaultOn.is_enabled(None));
    assert!(!DefaultOn.is_enabled(Some("off")));
    assert!(!DefaultOff.is_enabled(None));
    assert!(DefaultOff.is_enabled(Some("on")));
    assert_eq!(CollapsedThreads::from_config(""), Disabled);
}
