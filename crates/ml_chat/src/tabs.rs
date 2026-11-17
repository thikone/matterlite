// SPDX-License-Identifier: AGPL-3.0-only

//! The open chat tabs (MatterLite.md §10.7), independent of the view. A tab holds a chat or a
//! thread ("Open in tab" in the right panel). Opening one focuses
//! its tab or opens one next to the active tab; at most `cap` unpinned tabs stay open (the
//! least recently used one closes); pinned tabs stay together on the left and never close on
//! their own; closed tabs can be reopened; the list is saved per account.

use std::fmt;

use mm_model::{ChannelId, PostId};
use serde::{Deserialize, Serialize};

/// Characters of a thread's root post in its tab's title.
const THREAD_TITLE_CHARS: usize = 30;

/// A thread tab's title: the first line of its root post (whitespace collapsed), cut with an
/// ellipsis; "Thread" for an empty post.
pub fn thread_title(message: &str) -> String {
    let line = message
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default();
    let line: String = line.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.is_empty() {
        return "Thread".to_owned();
    }
    if line.chars().count() <= THREAD_TITLE_CHARS {
        return line;
    }
    let cut: String = line.chars().take(THREAD_TITLE_CHARS).collect();
    format!("{}…", cut.trim_end())
}

/// The default LRU cap (Settings → Chats).
pub const DEFAULT_MAX_TABS: usize = 12;
/// How many closed tabs Reopen remembers.
const CLOSED_KEPT: usize = 20;

/// What a chat view or tab shows: a channel, or a thread by its root post. Saved as
/// `{"channel": id}` (the form of tab lists saved before threads) or `{"thread": id}`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChatKey {
    Channel(ChannelId),
    Thread(PostId),
}

impl ChatKey {
    pub fn channel(&self) -> Option<&ChannelId> {
        match self {
            Self::Channel(channel) => Some(channel),
            Self::Thread(_) => None,
        }
    }

    pub fn thread(&self) -> Option<&PostId> {
        match self {
            Self::Channel(_) => None,
            Self::Thread(root) => Some(root),
        }
    }
}

impl fmt::Display for ChatKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Channel(channel) => write!(f, "{channel}"),
            Self::Thread(root) => write!(f, "thread:{root}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tab {
    #[serde(flatten)]
    pub key: ChatKey,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pinned: bool,
    /// When the tab was last active (a counter; larger is more recent).
    #[serde(skip)]
    used: u64,
}

/// What [`ChatTabs::open`] did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Opened {
    Focused,
    /// A new tab; the listed chats' tabs were closed to stay within the cap.
    New {
        closed: Vec<ChatKey>,
    },
}

#[derive(Clone, Debug, Default)]
pub struct ChatTabs {
    tabs: Vec<Tab>,
    active: Option<usize>,
    closed: Vec<Tab>,
    clock: u64,
    cap: usize,
}

/// The saved form.
#[derive(Serialize, Deserialize)]
struct Saved {
    tabs: Vec<Tab>,
    active: Option<SavedKey>,
}

/// The active tab as saved: a key, or a channel id alone (before threads).
#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum SavedKey {
    Key(ChatKey),
    Channel(ChannelId),
}

impl From<SavedKey> for ChatKey {
    fn from(saved: SavedKey) -> Self {
        match saved {
            SavedKey::Key(key) => key,
            SavedKey::Channel(channel) => Self::Channel(channel),
        }
    }
}

impl ChatTabs {
    pub fn new(cap: usize) -> Self {
        Self {
            cap: cap.max(1),
            ..Self::default()
        }
    }

    pub fn tabs(&self) -> &[Tab] {
        &self.tabs
    }

    pub fn active(&self) -> Option<usize> {
        self.active
    }

    pub fn active_key(&self) -> Option<&ChatKey> {
        self.active.map(|index| &self.tabs[index].key)
    }

    pub fn position(&self, key: &ChatKey) -> Option<usize> {
        self.tabs.iter().position(|tab| &tab.key == key)
    }

    pub fn set_cap(&mut self, cap: usize) -> Vec<ChatKey> {
        self.cap = cap.max(1);
        self.enforce_cap()
    }

    /// Focuses the chat's tab, or opens one right after the active tab (after the pinned ones
    /// when the active tab is pinned).
    pub fn open(&mut self, key: ChatKey) -> Opened {
        if let Some(index) = self.position(&key) {
            self.activate(index);
            return Opened::Focused;
        }
        let pinned = self.pinned_count();
        let at = match self.active {
            Some(active) => (active + 1).max(pinned),
            None => self.tabs.len(),
        };
        self.tabs.insert(
            at,
            Tab {
                key,
                pinned: false,
                used: 0,
            },
        );
        self.activate(at);
        Opened::New {
            closed: self.enforce_cap(),
        }
    }

    pub fn activate(&mut self, index: usize) {
        if index < self.tabs.len() {
            self.clock += 1;
            self.tabs[index].used = self.clock;
            self.active = Some(index);
        }
    }

    /// Closes a tab. When it was the active one, the tab to its right (or else left) becomes
    /// active, as in Warp.
    pub fn close(&mut self, index: usize) -> Option<ChatKey> {
        if index >= self.tabs.len() {
            return None;
        }
        let tab = self.tabs.remove(index);
        self.remember_closed(tab.clone());
        self.active = match self.active {
            _ if self.tabs.is_empty() => None,
            Some(active) if active == index => {
                let next = index.min(self.tabs.len() - 1);
                self.clock += 1;
                self.tabs[next].used = self.clock;
                Some(next)
            }
            Some(active) if active > index => Some(active - 1),
            other => other,
        };
        Some(tab.key)
    }

    /// Closes every unpinned tab but this one.
    pub fn close_others(&mut self, index: usize) -> Vec<ChatKey> {
        let Some(keep) = self.tabs.get(index).map(|tab| tab.key.clone()) else {
            return Vec::new();
        };
        let mut closed = Vec::new();
        let mut at = 0;
        while at < self.tabs.len() {
            if self.tabs[at].key != keep && !self.tabs[at].pinned {
                let tab = self.tabs.remove(at);
                closed.push(tab.key.clone());
                self.remember_closed(tab);
            } else {
                at += 1;
            }
        }
        let index = self.position(&keep).expect("kept");
        self.activate(index);
        closed
    }

    /// Reopens the most recently closed tab whose chat isn't open already.
    pub fn reopen(&mut self) -> Option<ChatKey> {
        while let Some(tab) = self.closed.pop() {
            if self.position(&tab.key).is_none() {
                let key = tab.key.clone();
                self.open(key.clone());
                if tab.pinned
                    && let Some(index) = self.position(&key)
                {
                    self.toggle_pin(index);
                }
                return Some(key);
            }
        }
        None
    }

    /// Pins a tab (it moves to the end of the pinned group) or unpins it (to the start of the
    /// unpinned ones). Returns its new index.
    pub fn toggle_pin(&mut self, index: usize) -> usize {
        if index >= self.tabs.len() {
            return index;
        }
        let active = self.active_key().cloned();
        let mut tab = self.tabs.remove(index);
        tab.pinned = !tab.pinned;
        let at = self.pinned_count();
        let key = tab.key.clone();
        self.tabs.insert(at, tab);
        self.active = active.and_then(|active| self.position(&active));
        self.position(&key).expect("just inserted")
    }

    /// Drops the tabs whose chat is gone (left, deleted for me). Returns them.
    pub fn retain(&mut self, mut keep: impl FnMut(&ChatKey) -> bool) -> Vec<ChatKey> {
        let active = self.active_key().cloned();
        let mut removed = Vec::new();
        self.tabs.retain(|tab| {
            let kept = keep(&tab.key);
            if !kept {
                removed.push(tab.key.clone());
            }
            kept
        });
        if !removed.is_empty() {
            self.active = match active.and_then(|active| self.position(&active)) {
                Some(index) => Some(index),
                // The most recently used remaining tab (none when no tab is left).
                None => self
                    .tabs
                    .iter()
                    .enumerate()
                    .max_by_key(|(_, tab)| tab.used)
                    .map(|(index, _)| index),
            };
        }
        removed
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(&Saved {
            tabs: self.tabs.clone(),
            active: self.active_key().cloned().map(SavedKey::Key),
        })
        .expect("tabs serialize")
    }

    /// The saved tabs whose chat still exists (`exists`), with the cap applied.
    pub fn from_json(json: &str, cap: usize, exists: impl Fn(&ChatKey) -> bool) -> Self {
        let mut tabs = Self::new(cap);
        let Ok(saved) = serde_json::from_str::<Saved>(json) else {
            return tabs;
        };
        let mut seen = std::collections::HashSet::new();
        tabs.tabs = saved
            .tabs
            .into_iter()
            .filter(|tab| exists(&tab.key) && seen.insert(tab.key.clone()))
            .collect();
        // Pinned first, keeping the order within each group.
        tabs.tabs.sort_by_key(|tab| !tab.pinned);
        if let Some(index) = saved
            .active
            .map(ChatKey::from)
            .and_then(|active| tabs.position(&active))
        {
            tabs.activate(index);
        } else if !tabs.tabs.is_empty() {
            tabs.activate(0);
        }
        tabs.enforce_cap();
        tabs
    }

    fn pinned_count(&self) -> usize {
        self.tabs.iter().take_while(|tab| tab.pinned).count()
    }

    fn remember_closed(&mut self, tab: Tab) {
        self.closed.retain(|closed| closed.key != tab.key);
        self.closed.push(tab);
        if self.closed.len() > CLOSED_KEPT {
            self.closed.remove(0);
        }
    }

    /// Closes least recently used unpinned tabs (never the active one) beyond the cap.
    fn enforce_cap(&mut self) -> Vec<ChatKey> {
        let mut closed = Vec::new();
        loop {
            let unpinned = self.tabs.iter().filter(|tab| !tab.pinned).count();
            if unpinned <= self.cap {
                return closed;
            }
            let victim = self
                .tabs
                .iter()
                .enumerate()
                .filter(|(index, tab)| !tab.pinned && Some(*index) != self.active)
                .min_by_key(|(_, tab)| tab.used)
                .map(|(index, _)| index);
            let Some(victim) = victim else {
                return closed;
            };
            let active = self.active_key().cloned();
            let tab = self.tabs.remove(victim);
            closed.push(tab.key.clone());
            self.remember_closed(tab);
            self.active = active.and_then(|active| self.position(&active));
        }
    }
}

#[cfg(test)]
#[path = "tabs_tests.rs"]
mod tests;
