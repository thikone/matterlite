// SPDX-License-Identifier: AGPL-3.0-only

//! The text of system messages (WebApp.md §12): built from the post's type and props with the
//! webapp's English wording (`components/post_markdown/system_message_helpers.tsx`), as
//! Markdown so names become mentions. Types without a rule use the server's `message`.

use mm_model::Post;

fn prop<'a>(post: &'a Post, key: &str) -> Option<&'a str> {
    post.props
        .get(key)
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty())
}

fn user(post: &Post, key: &str) -> Option<String> {
    prop(post, key).map(|name| format!("@{name}"))
}

/// The Markdown for a system message, or `None` to show `post.message`.
pub(crate) fn markdown(post: &Post) -> Option<String> {
    let username = user(post, "username");
    let by = |text: &str| username.as_ref().map(|name| format!("{name} {text}"));
    let change = |what: &str, old_key: &str, new_key: &str| {
        let name = username.as_ref()?;
        let old = prop(post, old_key);
        let new = prop(post, new_key);
        Some(match (old, new) {
            (Some(old), Some(new)) => {
                format!("{name} updated the channel {what} from: {old} to: {new}")
            }
            (None, Some(new)) => format!("{name} updated the channel {what} to: {new}"),
            (Some(old), None) => format!("{name} removed the channel {what} (was: {old})"),
            (None, None) => return None,
        })
    };
    match post.post_type.as_str() {
        "system_join_channel" => by("joined the channel."),
        "system_guest_join_channel" => by("joined the channel as a guest."),
        "system_leave_channel" => by("left the channel."),
        "system_add_to_channel" => Some(format!(
            "{} added to the channel by {}.",
            user(post, "addedUsername")?,
            username?
        )),
        "system_add_guest_to_chan" => Some(format!(
            "{} added to the channel as a guest by {}.",
            user(post, "addedUsername")?,
            username?
        )),
        "system_remove_from_channel" => Some(format!(
            "{} was removed from the channel",
            user(post, "removedUsername")?
        )),
        "system_join_team" => by("joined the team."),
        "system_leave_team" => by("left the team."),
        "system_add_to_team" => Some(format!(
            "{} added to the team by {}",
            user(post, "addedUsername")?,
            username?
        )),
        "system_remove_from_team" => Some(format!(
            "{} was removed from the team.",
            user(post, "removedUsername")?
        )),
        "system_header_change" => {
            // The header is Markdown itself; the webapp puts the old and new one on lines of
            // their own.
            let name = username.as_ref()?;
            match (prop(post, "old_header"), prop(post, "new_header")) {
                (Some(old), Some(new)) => Some(format!(
                    "{name} updated the channel header\n**From:** {old}\n**To:** {new}"
                )),
                _ => change("header", "old_header", "new_header"),
            }
        }
        "system_displayname_change" => change("display name", "old_displayname", "new_displayname"),
        "system_purpose_change" => change("purpose", "old_purpose", "new_purpose"),
        "system_convert_channel" | "system_change_chan_privacy" => {
            by("converted the channel from public to private")
        }
        "system_channel_deleted" => by("archived the channel."),
        "system_channel_restored" => by("unarchived the channel."),
        _ => None,
    }
}
