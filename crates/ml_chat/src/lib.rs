// SPDX-License-Identifier: AGPL-3.0-only

//! MatterLite's chat models and views on top of the sync engine (`mm_sync`) and the window
//! shell (`ml_workspace`): the signed-in [`Session`], the login screen and the window root.

pub mod account_theme;
pub mod chats;
mod chats_view;
mod format;
mod login_view;
mod root_view;
pub mod session;
pub mod settings;
mod switcher_view;
pub mod tabs;

pub use chats_view::{ChatsAction, ChatsEvent, ChatsView};
pub use login_view::{LoginAction, LoginView};
pub use root_view::{RootAction, RootView};
pub use session::{Session, SessionEvent, SessionState};

/// Registers the session model and the keybindings. Secure storage must be registered first.
pub fn init(app: &mut warpui::AppContext) {
    settings::ChatSettings::register(app);
    app.add_singleton_model(|_| Session::new(warp_core::paths::servers_dir()));
    app.add_singleton_model(account_theme::AccountThemeFollower::new);
    root_view::init(app);
}

/// Opens the most recently used account if its login is saved (call once the window exists,
/// so its first frame isn't held up by the cache).
pub fn restore_session(app: &mut warpui::AppContext) {
    use warpui::SingletonEntity as _;
    Session::handle(app).update(app, |session, ctx| session.restore(ctx));
}

/// Stops the sync engine, saving its state (at quit).
pub fn shutdown(app: &mut warpui::AppContext) {
    use warpui::SingletonEntity as _;
    Session::handle(app).update(app, |session, _| session.shutdown());
}

/// The window became active or inactive: chats are marked read only while it is active.
pub fn set_window_active(app: &mut warpui::AppContext, active: bool) {
    use warpui::SingletonEntity as _;
    Session::as_ref(app).send(mm_sync::Command::SetFocused(active));
}
