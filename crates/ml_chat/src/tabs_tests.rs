// SPDX-License-Identifier: AGPL-3.0-only

use super::*;

fn id(name: &str) -> ChatKey {
    ChatKey::Channel(ChannelId::from(name))
}

fn names(tabs: &ChatTabs) -> Vec<String> {
    tabs.tabs()
        .iter()
        .enumerate()
        .map(|(index, tab)| {
            let mut name = tab.key.to_string();
            if tab.pinned {
                name.insert(0, '^');
            }
            if tabs.active() == Some(index) {
                name.push('*');
            }
            name
        })
        .collect()
}

#[test]
fn opening_focuses_or_adds_next_to_the_active_tab() {
    let mut tabs = ChatTabs::new(12);
    assert_eq!(tabs.open(id("a")), Opened::New { closed: vec![] });
    tabs.open(id("b"));
    tabs.open(id("c"));
    assert_eq!(names(&tabs), ["a", "b", "c*"]);
    assert_eq!(tabs.open(id("a")), Opened::Focused);
    tabs.open(id("d"));
    assert_eq!(names(&tabs), ["a", "d*", "b", "c"]);
}

#[test]
fn the_least_recently_used_unpinned_tab_closes_beyond_the_cap() {
    let mut tabs = ChatTabs::new(3);
    for name in ["a", "b", "c"] {
        tabs.open(id(name));
    }
    tabs.toggle_pin(0); // a pinned: not counted, never closed
    tabs.open(id("b")); // b used more recently than c
    assert_eq!(names(&tabs), ["^a", "b*", "c"]);
    tabs.open(id("d"));
    // e makes four unpinned tabs: c, used longest ago, closes.
    assert_eq!(
        tabs.open(id("e")),
        Opened::New {
            closed: vec![id("c")]
        }
    );
    assert_eq!(names(&tabs), ["^a", "b", "d", "e*"]);
    assert_eq!(
        tabs.open(id("f")),
        Opened::New {
            closed: vec![id("b")]
        }
    );
    assert_eq!(names(&tabs), ["^a", "d", "e", "f*"]);
    // A lower cap closes at once, never the active tab.
    assert_eq!(tabs.set_cap(1), vec![id("d"), id("e")]);
    assert_eq!(names(&tabs), ["^a", "f*"]);
}

#[test]
fn closing_activates_the_neighbour_and_reopen_brings_it_back() {
    let mut tabs = ChatTabs::new(12);
    for name in ["a", "b", "c"] {
        tabs.open(id(name));
    }
    tabs.activate(1);
    assert_eq!(tabs.close(1), Some(id("b")));
    assert_eq!(names(&tabs), ["a", "c*"]);
    tabs.close(1);
    assert_eq!(names(&tabs), ["a*"]);
    assert_eq!(tabs.reopen(), Some(id("c")));
    assert_eq!(tabs.reopen(), Some(id("b")));
    assert_eq!(names(&tabs), ["a", "c", "b*"]);
    assert_eq!(tabs.reopen(), None);
    tabs.close(0);
    tabs.close(0);
    tabs.close(0);
    assert_eq!(tabs.active(), None);
}

#[test]
fn pinning_moves_tabs_into_the_pinned_group() {
    let mut tabs = ChatTabs::new(12);
    for name in ["a", "b", "c"] {
        tabs.open(id(name));
    }
    assert_eq!(tabs.toggle_pin(2), 0);
    assert_eq!(names(&tabs), ["^c*", "a", "b"]);
    assert_eq!(tabs.toggle_pin(2), 1);
    assert_eq!(names(&tabs), ["^c*", "^b", "a"]);
    // New tabs never go between pinned ones.
    tabs.open(id("d"));
    assert_eq!(names(&tabs), ["^c", "^b", "d*", "a"]);
    assert_eq!(tabs.close_others(2), vec![id("a")]);
    assert_eq!(names(&tabs), ["^c", "^b", "d*"]);
    assert_eq!(tabs.toggle_pin(0), 1);
    assert_eq!(names(&tabs), ["^b", "c", "d*"]);
}

#[test]
fn saved_tabs_come_back_without_gone_chats() {
    let mut tabs = ChatTabs::new(12);
    for name in ["a", "b", "c"] {
        tabs.open(id(name));
    }
    tabs.toggle_pin(2);
    tabs.activate(2);
    let json = tabs.to_json();
    let restored = ChatTabs::from_json(&json, 12, |channel| *channel != id("a"));
    assert_eq!(names(&restored), ["^c", "b*"]);
    let restored = ChatTabs::from_json(&json, 12, |channel| *channel != id("b"));
    assert_eq!(names(&restored), ["^c*", "a"]);
    assert!(
        ChatTabs::from_json("garbage", 12, |_| true)
            .tabs()
            .is_empty()
    );
}

#[test]
fn chats_that_disappear_lose_their_tabs() {
    let mut tabs = ChatTabs::new(12);
    for name in ["a", "b", "c"] {
        tabs.open(id(name));
    }
    tabs.activate(0);
    tabs.activate(1);
    assert_eq!(tabs.retain(|channel| *channel != id("b")), vec![id("b")]);
    // The most recently used remaining tab becomes active.
    assert_eq!(names(&tabs), ["a*", "c"]);
}

#[test]
fn threads_have_tabs_too_and_old_lists_still_load() {
    let mut tabs = ChatTabs::new(12);
    tabs.open(id("a"));
    let thread = ChatKey::Thread("root".into());
    tabs.open(thread.clone());
    assert_eq!(names(&tabs), ["a", "thread:root*"]);
    // Focus-or-open works for threads as for chats.
    tabs.open(id("a"));
    assert_eq!(tabs.open(thread.clone()), Opened::Focused);

    let json = tabs.to_json();
    assert!(json.contains(r#"{"thread":"root"}"#), "{json}");
    let restored = ChatTabs::from_json(&json, 12, |_| true);
    assert_eq!(names(&restored), ["a", "thread:root*"]);

    // A list saved before threads: tabs by channel, the active one as a plain id.
    let old = r#"{"tabs":[{"channel":"a","pinned":true},{"channel":"b"}],"active":"b"}"#;
    let restored = ChatTabs::from_json(old, 12, |_| true);
    assert_eq!(names(&restored), ["^a", "b*"]);
}

#[test]
fn thread_titles_are_the_start_of_the_root() {
    assert_eq!(thread_title("Ship it?"), "Ship it?");
    assert_eq!(thread_title("\n  Release   plan\nmore"), "Release plan");
    assert_eq!(
        thread_title("Thread root: should we ship the WebSocket resume fix today?"),
        "Thread root: should we ship th…"
    );
    assert_eq!(thread_title("   "), "Thread");
}
