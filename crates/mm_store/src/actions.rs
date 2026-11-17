// SPDX-License-Identifier: AGPL-3.0-only

//! What I may do with a post: the entries of the webapp's post menu
//! (`components/dot_menu/dot_menu.tsx`, `canEditPost` and `canDeletePost` in
//! `post_utils.ts`), computed from the store.

use mm_model::PostId;
use mm_model::preference::keys;

use crate::Store;

/// The actions offered for a post. All false for a pending, deleted or system post.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PostActions {
    pub reply: bool,
    /// Add a reaction (a chip, or the picker when `emoji_picker`) and remove mine.
    pub react: bool,
    pub unreact: bool,
    /// The emoji picker is on (`EnableEmojiPicker`): React in the toolbar and the `+` chip.
    pub emoji_picker: bool,
    /// Follow or unfollow the post's thread (CRT only); `following` is the current state.
    pub follow: bool,
    pub following: bool,
    pub mark_unread: bool,
    pub save: bool,
    pub saved: bool,
    pub pin: bool,
    pub pinned: bool,
    pub copy: bool,
    pub edit: bool,
    pub delete: bool,
}

impl PostActions {
    /// Whether anything is offered (a post without actions shows no toolbar).
    pub fn any(&self) -> bool {
        *self != Self::default()
    }
}

impl Store {
    /// The actions on a post at `now_ms` (the edit time limit depends on it).
    pub fn post_actions(&self, post: &PostId, now_ms: i64) -> PostActions {
        let Some(post) = self.posts.get(post) else {
            return PostActions::default();
        };
        if post.is_deleted()
            || post.is_system()
            || self.pending.contains_key(post.id.as_str())
            || !post.pending_post_id.is_empty() && self.pending.contains_key(&post.pending_post_id)
        {
            return PostActions::default();
        }
        let channel = &post.channel_id;
        let archived = self
            .channels
            .get(channel)
            .is_none_or(|channel| channel.delete_at != 0);
        let mine = post.user_id == self.me;
        let can = |permission: &str| self.has_channel_permission(channel, permission);
        let root = if post.is_root() {
            &post.id
        } else {
            &post.root_id
        };
        let following = self
            .posts
            .get(root)
            .and_then(|root| root.is_following)
            .unwrap_or_else(|| self.thread_states.contains_key(root));
        let within_edit_limit = || {
            // Enforced by the server only with a licence (`canEditPost`); -1 means no limit.
            let limit = self.config.int("PostEditTimeLimit").unwrap_or(-1);
            self.license.get("IsLicensed") != Some("true")
                || limit < 0
                || now_ms < post.create_at + limit * 1000
        };
        let edit = !archived
            && if mine {
                can("edit_post") && within_edit_limit()
            } else {
                can("edit_others_posts")
            };
        let delete = !archived
            && if mine {
                can("delete_post")
            } else {
                can("delete_others_posts")
            };
        let react = !archived && can("add_reaction");
        PostActions {
            reply: !archived && can("create_post"),
            react,
            unreact: !archived && can("remove_reaction"),
            emoji_picker: react && self.config.get("EnableEmojiPicker") != Some("false"),
            follow: self.crt_enabled(),
            following,
            mark_unread: !archived,
            save: true,
            saved: self.preference(keys::FLAGGED_POST, post.id.as_str()) == Some("true"),
            pin: !archived,
            pinned: post.is_pinned,
            copy: true,
            edit,
            delete,
        }
    }
}

#[cfg(test)]
#[path = "actions_tests.rs"]
mod tests;
