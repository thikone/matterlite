// SPDX-License-Identifier: AGPL-3.0-only

use ml_themes::ColorU;

use super::*;

fn store() -> Store {
    Store::default()
}

fn set(store: &mut Store, name: &str, json: &str) {
    store
        .preferences
        .insert((keys::THEME.to_owned(), name.to_owned()), json.to_owned());
}

#[test]
fn falls_back_to_the_server_default_then_denim() {
    let mut store = store();
    let theme = account_theme(&store, None);
    assert_eq!(theme.name().as_deref(), Some("Mattermost account: Denim"));
    store
        .config
        .0
        .insert("DefaultTheme".to_owned(), "onyx".to_owned());
    assert_eq!(
        account_theme(&store, None).name().as_deref(),
        Some("Mattermost account: Onyx")
    );
    store
        .config
        .0
        .insert("DefaultTheme".to_owned(), "nonsense".to_owned());
    assert_eq!(
        account_theme(&store, None).name().as_deref(),
        Some("Mattermost account: Denim")
    );
}

#[test]
fn the_team_theme_wins_over_the_global_one() {
    let mut store = store();
    let team = TeamId::new("t".repeat(26));
    set(&mut store, "", mattermost::INDIGO);
    assert_eq!(
        account_theme(&store, Some(&team)).name().as_deref(),
        Some("Mattermost account: Indigo")
    );
    set(
        &mut store,
        team.as_str(),
        r##"{"type":"custom","sidebarBg":"#123456"}"##,
    );
    let theme = account_theme(&store, Some(&team));
    assert_eq!(theme.name().as_deref(), Some("Mattermost account: custom"));
    assert_eq!(theme.chat().sidebar_bg, ColorU::from_u32(0x123456FF));
    // Another team uses the global one; a broken preference falls back to the default.
    let other = TeamId::new("o".repeat(26));
    assert_eq!(
        account_theme(&store, Some(&other)).name().as_deref(),
        Some("Mattermost account: Indigo")
    );
    set(&mut store, "", "{broken");
    assert_eq!(
        account_theme(&store, None).name().as_deref(),
        Some("Mattermost account: Denim")
    );
}
