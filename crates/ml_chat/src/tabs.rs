// SPDX-License-Identifier: AGPL-3.0-only

//! The open chat tabs (MatterLite.md §10.7), independent of the view: opening a chat focuses
//! its tab or opens one next to the active tab; at most `cap` unpinned tabs stay open (the
//! least recently used one closes); pinned tabs stay together on the left and never close on
//! their own; closed tabs can be reopened; the list is saved per account.

use mm_model::ChannelId;
use serde::{Deserialize, Serialize};

/// The default LRU cap (Settings → Chats).
pub const DEFAULT_MAX_TABS: usize = 12;
/// How many closed tabs Reopen remembers.
const CLOSED_KEPT: usize = 20;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tab {
    pub channel: ChannelId,
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
        closed: Vec<ChannelId>,
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
    active: Option<ChannelId>,
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

    pub fn active_channel(&self) -> Option<&ChannelId> {
        self.active.map(|index| &self.tabs[index].channel)
    }

    pub fn position(&self, channel: &ChannelId) -> Option<usize> {
        self.tabs.iter().position(|tab| &tab.channel == channel)
    }

    pub fn set_cap(&mut self, cap: usize) -> Vec<ChannelId> {
        self.cap = cap.max(1);
        self.enforce_cap()
    }

    /// Focuses the chat's tab, or opens one right after the active tab (after the pinned ones
    /// when the active tab is pinned).
    pub fn open(&mut self, channel: ChannelId) -> Opened {
        if let Some(index) = self.position(&channel) {
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
                channel,
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
    pub fn close(&mut self, index: usize) -> Option<ChannelId> {
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
        Some(tab.channel)
    }

    /// Closes every unpinned tab but this one.
    pub fn close_others(&mut self, index: usize) -> Vec<ChannelId> {
        let Some(keep) = self.tabs.get(index).map(|tab| tab.channel.clone()) else {
            return Vec::new();
        };
        let mut closed = Vec::new();
        let mut at = 0;
        while at < self.tabs.len() {
            if self.tabs[at].channel != keep && !self.tabs[at].pinned {
                let tab = self.tabs.remove(at);
                closed.push(tab.channel.clone());
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
    pub fn reopen(&mut self) -> Option<ChannelId> {
        while let Some(tab) = self.closed.pop() {
            if self.position(&tab.channel).is_none() {
                let channel = tab.channel.clone();
                self.open(channel.clone());
                if tab.pinned
                    && let Some(index) = self.position(&channel)
                {
                    self.toggle_pin(index);
                }
                return Some(channel);
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
        let active = self.active_channel().cloned();
        let mut tab = self.tabs.remove(index);
        tab.pinned = !tab.pinned;
        let at = self.pinned_count();
        let channel = tab.channel.clone();
        self.tabs.insert(at, tab);
        self.active = active.and_then(|active| self.position(&active));
        self.position(&channel).expect("just inserted")
    }

    /// Drops the tabs whose chat is gone (left, deleted for me). Returns them.
    pub fn retain(&mut self, mut keep: impl FnMut(&ChannelId) -> bool) -> Vec<ChannelId> {
        let active = self.active_channel().cloned();
        let mut removed = Vec::new();
        self.tabs.retain(|tab| {
            let kept = keep(&tab.channel);
            if !kept {
                removed.push(tab.channel.clone());
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
            active: self.active_channel().cloned(),
        })
        .expect("tabs serialize")
    }

    /// The saved tabs whose chat still exists (`exists`), with the cap applied.
    pub fn from_json(json: &str, cap: usize, exists: impl Fn(&ChannelId) -> bool) -> Self {
        let mut tabs = Self::new(cap);
        let Ok(saved) = serde_json::from_str::<Saved>(json) else {
            return tabs;
        };
        let mut seen = std::collections::HashSet::new();
        tabs.tabs = saved
            .tabs
            .into_iter()
            .filter(|tab| exists(&tab.channel) && seen.insert(tab.channel.clone()))
            .collect();
        // Pinned first, keeping the order within each group.
        tabs.tabs.sort_by_key(|tab| !tab.pinned);
        if let Some(index) = saved.active.and_then(|active| tabs.position(&active)) {
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
        self.closed.retain(|closed| closed.channel != tab.channel);
        self.closed.push(tab);
        if self.closed.len() > CLOSED_KEPT {
            self.closed.remove(0);
        }
    }

    /// Closes least recently used unpinned tabs (never the active one) beyond the cap.
    fn enforce_cap(&mut self) -> Vec<ChannelId> {
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
            let active = self.active_channel().cloned();
            let tab = self.tabs.remove(victim);
            closed.push(tab.channel.clone());
            self.remember_closed(tab);
            self.active = active.and_then(|active| self.position(&active));
        }
    }
}

#[cfg(test)]
#[path = "tabs_tests.rs"]
mod tests;
