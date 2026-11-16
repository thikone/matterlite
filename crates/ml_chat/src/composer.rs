// SPDX-License-Identifier: AGPL-3.0-only

//! The composer's rules, independent of the view (WebApp.md §15, MatterLite.md §13): what
//! Enter does (`postMessageOnKeyPress` and `isWithinCodeBlock` in `CS/utils/post_utils.ts`),
//! the length limit, and the typing indicator (`MsgTyping`, `userTyping` in
//! `CS/actions/global_actions.tsx`).

use std::time::{Duration, Instant};

use mm_model::{ChannelId, PostId, UserId};
use mm_store::Store;

/// `Constants.DEFAULT_CHARACTER_LIMIT`, when the server doesn't say (`MaxPostSize`).
pub const DEFAULT_MAX_POST_SIZE: usize = 4000;
/// `TimeBetweenUserTypingUpdatesMilliseconds` when the server doesn't say.
pub const DEFAULT_TYPING_INTERVAL: Duration = Duration::from_millis(5000);

/// The Enter preferences (Settings › Advanced in the webapp).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EnterPrefs {
    /// `advanced_settings/send_on_ctrl_enter` (default off): Enter adds a line, Ctrl/Cmd+Enter
    /// sends.
    pub send_on_ctrl_enter: bool,
    /// `advanced_settings/code_block_ctrl_enter` (default on): inside an open ``` block, Enter
    /// adds a line and Ctrl/Cmd+Enter sends (closing the block).
    pub code_block_ctrl_enter: bool,
}

impl EnterPrefs {
    pub fn of(store: &Store) -> Self {
        let flag = |name: &str, default: bool| {
            store
                .preference("advanced_settings", name)
                .map_or(default, |value| value == "true")
        };
        Self {
            send_on_ctrl_enter: flag("send_on_ctrl_enter", false),
            code_block_ctrl_enter: flag("code_block_ctrl_enter", true),
        }
    }
}

impl Default for EnterPrefs {
    fn default() -> Self {
        Self {
            send_on_ctrl_enter: false,
            code_block_ctrl_enter: true,
        }
    }
}

/// Enter alone, or with Ctrl (Cmd on macOS). Shift+Enter and Alt+Enter always add a line and
/// never get here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EnterKey {
    Plain,
    CtrlOrCmd,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OnEnter {
    /// Send this text (with a code block closed if Ctrl+Enter sent from inside one).
    Send(String),
    NewLine,
}

/// What Enter does, as `postMessageOnKeyPress`. `caret` is a byte offset into `message`.
pub fn on_enter(message: &str, caret: usize, key: EnterKey, prefs: EnterPrefs) -> OnEnter {
    if !prefs.send_on_ctrl_enter && !prefs.code_block_ctrl_enter {
        return OnEnter::Send(message.to_owned());
    }
    let ctrl = key == EnterKey::CtrlOrCmd;
    if prefs.send_on_ctrl_enter {
        send_on_ctrl_enter(message, ctrl, true, caret)
    } else if message.trim().is_empty() {
        OnEnter::Send(message.to_owned())
    } else {
        send_on_ctrl_enter(message, ctrl, false, caret)
    }
}

fn send_on_ctrl_enter(message: &str, ctrl: bool, send_on_ctrl: bool, caret: usize) -> OnEnter {
    let in_code_block = within_code_block(message, caret);
    if (send_on_ctrl && ctrl && !in_code_block) || (!send_on_ctrl && !in_code_block) {
        OnEnter::Send(message.to_owned())
    } else if ctrl && in_code_block {
        OnEnter::Send(close_code_block(message))
    } else {
        OnEnter::NewLine
    }
}

/// Whether the text before `caret` opens more ``` fences than it closes
/// (`REGEX_CODE_BLOCK_OPTIONAL_LANGUAGE_TAG` is `/^```.*$/gm`).
pub fn within_code_block(message: &str, caret: usize) -> bool {
    let before = &message[..floor_char_boundary(message, caret)];
    before
        .split('\n')
        .filter(|line| line.starts_with("```"))
        .count()
        % 2
        == 1
}

/// `canAutomaticallyCloseBackticks`: a block with content whose last line isn't a fence gets
/// its closing fence.
fn close_code_block(message: &str) -> String {
    let lines: Vec<&str> = message
        .split('\n')
        .filter(|line| !line.trim().is_empty())
        .collect();
    match lines.last() {
        Some(last) if lines.len() > 1 && !last.contains("```") => {
            if message.ends_with('\n') {
                format!("{message}```")
            } else {
                format!("{message}\n```")
            }
        }
        _ => message.to_owned(),
    }
}

fn floor_char_boundary(text: &str, index: usize) -> usize {
    let mut index = index.min(text.len());
    while !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

/// The server's `MaxPostSize`, else the webapp's default.
pub fn max_post_size(store: &Store) -> usize {
    store
        .config
        .get("MaxPostSize")
        .and_then(|value| value.parse().ok())
        .filter(|size| *size > 0)
        .unwrap_or(DEFAULT_MAX_POST_SIZE)
}

/// The error under the composer when the text is too long to send
/// (`create_post.error_message`), counted in characters as the webapp does.
pub fn length_error(message: &str, max: usize) -> Option<String> {
    let length = message.chars().count();
    (length > max).then(|| format!("Your message is too long. Character count: {length}/{max}"))
}

/// Whether typing in `channel` is announced: the server's `EnableUserTypingMessages`, and
/// fewer members than `MaxNotificationsPerChannel` (when the member count is known).
pub fn typing_enabled(store: &Store, channel: &ChannelId) -> bool {
    if store.config.get("EnableUserTypingMessages") == Some("false") {
        return false;
    }
    let limit = store
        .config
        .get("MaxNotificationsPerChannel")
        .and_then(|value| value.parse::<i64>().ok());
    match (limit, store.channel_stats.get(channel)) {
        (Some(limit), Some(stats)) => stats.member_count < limit,
        _ => true,
    }
}

/// `TimeBetweenUserTypingUpdatesMilliseconds`: how often typing is sent, and how long it shows.
pub fn typing_interval(store: &Store) -> Duration {
    store
        .config
        .get("TimeBetweenUserTypingUpdatesMilliseconds")
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|ms| *ms > 0)
        .map_or(DEFAULT_TYPING_INTERVAL, Duration::from_millis)
}

/// Sends typing at most once per interval.
#[derive(Debug, Default)]
pub struct TypingThrottle {
    last: Option<Instant>,
}

impl TypingThrottle {
    pub fn due(&mut self, now: Instant, interval: Duration) -> bool {
        if self
            .last
            .is_some_and(|last| now.duration_since(last) < interval)
        {
            return false;
        }
        self.last = Some(now);
        true
    }

    /// After sending a post, the next keystroke announces typing again.
    pub fn reset(&mut self) {
        self.last = None;
    }
}

/// Who is typing where: an entry lasts one interval after the last `typing` event, and ends
/// when that user posts there.
#[derive(Debug, Default)]
pub struct TypingUsers {
    entries: Vec<(ChannelId, PostId, UserId, Instant)>,
}

impl TypingUsers {
    pub fn add(&mut self, channel: ChannelId, root: PostId, user: UserId, now: Instant) {
        self.entries
            .retain(|(c, r, u, _)| !(*c == channel && *r == root && *u == user));
        self.entries.push((channel, root, user, now));
    }

    /// The user posted in the channel (or a thread of it).
    pub fn posted(&mut self, channel: &ChannelId, user: &UserId) -> bool {
        let before = self.entries.len();
        self.entries
            .retain(|(c, _, u, _)| !(c == channel && u == user));
        self.entries.len() != before
    }

    /// Drops entries older than `interval`; whether any went.
    pub fn expire(&mut self, now: Instant, interval: Duration) -> bool {
        let before = self.entries.len();
        self.entries
            .retain(|(_, _, _, at)| now.duration_since(*at) < interval);
        self.entries.len() != before
    }

    /// Typing in the channel's composer (`root` empty) or a thread, in the order they started.
    pub fn users(&self, channel: &ChannelId, root: &PostId) -> Vec<UserId> {
        self.entries
            .iter()
            .filter(|(c, r, _, _)| c == channel && r == root)
            .map(|(_, _, user, _)| user.clone())
            .collect()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// "carol is typing...", "bob and carol are typing...", "bob, carol and dave are typing..."
/// (`msg_typing.isTyping`, `msg_typing.areTyping`).
pub fn typing_text(names: &[String]) -> Option<String> {
    match names {
        [] => None,
        [one] => Some(format!("{one} is typing...")),
        [rest @ .., last] => Some(format!("{} and {last} are typing...", rest.join(", "))),
    }
}

#[cfg(test)]
#[path = "composer_tests.rs"]
mod tests;
