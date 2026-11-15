// SPDX-License-Identifier: AGPL-3.0-only

//! Follows the signed-in account's Mattermost theme (MatterLite.md §11.4): the `theme`
//! preference of the current team, else the one for all teams, else the server's
//! `DefaultTheme`, else Mattermost Denim. The result goes to the [`AppearanceManager`], which
//! shows it while the theme setting is "Mattermost account" (the default).

use ml_themes::{AppearanceManager, WarpTheme, mattermost};
use mm_model::TeamId;
use mm_model::preference::keys;
use mm_store::{Store, StoreDelta};
use warpui::{Entity, ModelContext, SingletonEntity};

use crate::session::{Session, SessionEvent};

/// The theme of the account in `store`, for `team` (the one shown in the Chats tab).
pub fn account_theme(store: &Store, team: Option<&TeamId>) -> WarpTheme {
    let preference = team
        .and_then(|team| store.preference(keys::THEME, team.as_str()))
        .or_else(|| store.preference(keys::THEME, ""))
        .filter(|json| !json.trim().is_empty());
    let mut theme = preference
        .and_then(|json| match mattermost::from_mattermost_json(json) {
            Ok(theme) => Some(theme),
            Err(error) => {
                log::warn!("the account's theme preference: {error:#}");
                None
            }
        })
        .unwrap_or_else(|| {
            let key = store
                .config
                .get("DefaultTheme")
                .filter(|key| mattermost::builtin_json(key).is_some())
                .unwrap_or(mattermost::DEFAULT_KEY);
            mattermost::from_mattermost_json(mattermost::builtin_json(key).expect("known key"))
                .expect("built-in themes parse")
        });
    let kind = theme.name().unwrap_or_default();
    theme.set_name(format!("Mattermost account: {kind}"));
    theme
}

/// Keeps the [`AppearanceManager`]'s account theme in step with the session.
pub struct AccountThemeFollower {
    team: Option<TeamId>,
}

impl AccountThemeFollower {
    pub fn new(ctx: &mut ModelContext<Self>) -> Self {
        ctx.subscribe_to_model(&Session::handle(ctx), |me, _, event, ctx| {
            let relevant = match event {
                SessionEvent::StateChanged | SessionEvent::Reloaded => true,
                SessionEvent::Changed(deltas) => deltas
                    .iter()
                    .any(|delta| matches!(delta, StoreDelta::PreferencesChanged)),
                _ => false,
            };
            if relevant {
                me.update(ctx);
            }
        });
        Self { team: None }
    }

    /// The team whose theme applies (team switcher, M3 step 3).
    pub fn set_team(&mut self, team: Option<TeamId>, ctx: &mut ModelContext<Self>) {
        if self.team != team {
            self.team = team;
            self.update(ctx);
        }
    }

    fn update(&mut self, ctx: &mut ModelContext<Self>) {
        let theme = Session::as_ref(ctx).store().and_then(|store| {
            store
                .read()
                .ok()
                .map(|store| account_theme(&store, self.team.as_ref()))
        });
        AppearanceManager::handle(ctx)
            .update(ctx, |manager, ctx| manager.set_account_theme(theme, ctx));
    }
}

impl Entity for AccountThemeFollower {
    type Event = ();
}

impl SingletonEntity for AccountThemeFollower {}

#[cfg(test)]
#[path = "account_theme_tests.rs"]
mod tests;
