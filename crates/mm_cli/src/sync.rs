// SPDX-License-Identifier: AGPL-3.0-only

//! `mm-cli <server> sync --data <dir>`: the sync engine, headless (docs/M2.md). Commands on
//! stdin: `open|older|newer|show <channel>`, `close`, `send <channel> <text>`, `unread`,
//! `search <words>`, `quit`.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context as _, Result};
use mm_api::Client;
use mm_model::{ChannelId, ChannelType, PostId};
use mm_store::{ItemOptions, Store, StoreDelta, TimelineItem};
use mm_sync::{Command, EngineConfig, Event};

pub(crate) async fn run(client: &Client, data: PathBuf) -> Result<()> {
    let me = client.me().await?;
    let token = client.token().context("not logged in")?.clone();
    let servers_dir = data.join("servers");
    let mut config = EngineConfig::new(client.server().clone(), token, servers_dir.clone());
    config.user_id = Some(me.id.clone());
    let started = Instant::now();
    let engine = mm_sync::start(config);
    println!(
        "sync engine for @{} (cache under {})",
        me.username,
        servers_dir.display()
    );

    // stdin lines, read on a thread.
    let (line_tx, line_rx) = async_channel::unbounded::<String>();
    std::thread::spawn(move || {
        let mut line = String::new();
        while std::io::stdin()
            .read_line(&mut line)
            .is_ok_and(|read| read > 0)
        {
            if line_tx.send_blocking(line.trim().to_owned()).is_err() {
                break;
            }
            line.clear();
        }
    });
    let mut opened_at: BTreeMap<ChannelId, i64> = BTreeMap::new();
    let mut stdin_open = true;
    loop {
        tokio::select! {
            event = engine.events().recv() => {
                let Ok(event) = event else { break };
                let time = format!("[{:>7.2}s]", started.elapsed().as_secs_f64());
                let store = engine.store().read().expect("store lock");
                match &event {
                    Event::Hydrated => println!(
                        "{time} hydrated from the cache: {} channels, {} posts",
                        store.channels.len(),
                        store.posts.len()
                    ),
                    Event::Ready { resumed } => {
                        println!(
                            "{time} ready ({})",
                            if *resumed { "resumed the saved session, no bootstrap" } else { "bootstrapped" }
                        );
                        print_badges(&store);
                    }
                    Event::Changed(deltas) => println!("{time} {}", summarize(&store, deltas)),
                    Event::ChannelOpened { channel, last_viewed_at } => {
                        opened_at.insert(channel.clone(), *last_viewed_at);
                        println!("{time} opened {}", name(&store, channel));
                    }
                    Event::Stopped => {
                        println!("{time} stopped");
                        break;
                    }
                    other => println!("{time} {other:?}"),
                }
            }
            line = line_rx.recv(), if stdin_open => {
                let Ok(line) = line else {
                    stdin_open = false;
                    continue;
                };
                if line == "quit" {
                    engine.send(Command::Shutdown);
                    continue;
                }
                let store = engine.store().read().expect("store lock");
                match parse_command(&store, &line) {
                    Ok(Action::Engine(command)) => engine.send(command),
                    Ok(Action::Show(channel)) => print_timeline(&store, &channel, opened_at.get(&channel).copied()),
                    Ok(Action::Unread) => print_badges(&store),
                    Ok(Action::Search(words)) => {
                        drop(store);
                        search(&engine, &servers_dir, &words);
                    }
                    Err(error) => println!("? {error}"),
                }
            }
            _ = tokio::signal::ctrl_c() => engine.send(Command::Shutdown),
        }
    }
    engine.shutdown();
    Ok(())
}

enum Action {
    Engine(Command),
    Show(ChannelId),
    Unread,
    Search(String),
}

fn parse_command(store: &Store, line: &str) -> Result<Action> {
    let (verb, rest) = line.split_once(' ').unwrap_or((line, ""));
    let channel =
        |text: &str| find_channel(store, text).with_context(|| format!("no channel `{text}`"));
    Ok(match verb {
        "open" => Action::Engine(Command::OpenChannel(channel(rest)?)),
        "older" => Action::Engine(Command::LoadOlder(channel(rest)?)),
        "newer" => Action::Engine(Command::LoadNewer(channel(rest)?)),
        "send" => {
            let (target, text) = rest.split_once(' ').context("send <channel> <text>")?;
            Action::Engine(Command::SendPost {
                channel: channel(target)?,
                root: PostId::default(),
                message: text.to_owned(),
            })
        }
        "show" => Action::Show(channel(rest)?),
        "close" => Action::Engine(Command::CloseChannel),
        "unread" => Action::Unread,
        "search" => Action::Search(rest.to_owned()),
        _ => anyhow::bail!(
            "commands: open|older|newer|show <channel>, close, send <channel> <text>, unread,              search <words>, quit"
        ),
    })
}

/// `dev` (a channel name) or `@bob` (a DM).
fn find_channel(store: &Store, text: &str) -> Option<ChannelId> {
    if let Some(username) = text.strip_prefix('@') {
        let user = store
            .users
            .values()
            .find(|user| user.username == username)?;
        return store
            .channels
            .values()
            .find(|channel| {
                channel.channel_type == ChannelType::Direct
                    && mm_store::dm_partner(&channel.name, &store.me).as_ref() == Some(&user.id)
            })
            .map(|channel| channel.id.clone());
    }
    store
        .channels
        .values()
        .find(|channel| channel.name == text)
        .map(|channel| channel.id.clone())
}

fn name(store: &Store, channel: &ChannelId) -> String {
    let label = store
        .channel_name(channel)
        .unwrap_or_else(|| channel.to_string());
    match store
        .channels
        .get(channel)
        .map(|channel| &channel.channel_type)
    {
        Some(ChannelType::Direct) => format!("@{label}"),
        Some(ChannelType::Group) => label,
        _ => format!("~{label}"),
    }
}

/// One line per change batch: counts by kind, names for channels.
fn summarize(store: &Store, deltas: &[StoreDelta]) -> String {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    let mut channels = Vec::new();
    for delta in deltas {
        let kind = match delta {
            StoreDelta::PostsChanged(ids) => {
                *counts.entry("posts").or_default() += ids.len();
                continue;
            }
            StoreDelta::PostsRemoved(ids) => {
                *counts.entry("removed").or_default() += ids.len();
                continue;
            }
            StoreDelta::TimelineChanged(channel) => {
                channels.push(format!("timeline {}", name(store, channel)));
                continue;
            }
            StoreDelta::MarkedRead {
                channel,
                notify_server,
            } => {
                channels.push(format!(
                    "read {}{}",
                    name(store, channel),
                    if *notify_server {
                        " (told the server)"
                    } else {
                        ""
                    }
                ));
                continue;
            }
            StoreDelta::MemberChanged(_) => "memberships",
            StoreDelta::ChannelChanged(_) => "channels",
            StoreDelta::UserChanged(_) => "users",
            StoreDelta::StatusChanged(_) => "statuses",
            StoreDelta::Typing { user, .. } => {
                channels.push(format!(
                    "typing: {}",
                    store.user_name(user).unwrap_or_else(|| user.to_string())
                ));
                continue;
            }
            StoreDelta::PendingChanged(_) => "pending",
            other => {
                channels.push(
                    format!("{other:?}")
                        .split(['(', ' '])
                        .next()
                        .unwrap_or("")
                        .to_owned(),
                );
                continue;
            }
        };
        *counts.entry(kind).or_default() += 1;
    }
    let mut parts: Vec<String> = counts
        .into_iter()
        .map(|(kind, count)| format!("{count} {kind}"))
        .collect();
    channels.dedup();
    parts.extend(channels);
    format!("changed: {}", parts.join(", "))
}

fn print_badges(store: &Store) {
    let mut teams: Vec<_> = store.teams.values().collect();
    teams.sort_by(|a, b| a.display_name.cmp(&b.display_name));
    for team in teams {
        let unread = store.team_unread(&team.id);
        println!(
            "           [{}] {} unread, {} mentions; threads {} unread",
            team.display_name, unread.messages, unread.mentions, unread.threads
        );
    }
    let direct = store.direct_unread();
    println!(
        "           [DMs/GMs] {} unread, {} mentions",
        direct.messages, direct.mentions
    );
}

fn print_timeline(store: &Store, channel: &ChannelId, last_viewed_at: Option<i64>) {
    let items = store.timeline_items(channel, &ItemOptions { last_viewed_at });
    println!(
        "  {} ({} items, newest last):",
        name(store, channel),
        items.len()
    );
    for item in items.iter().take(12).rev() {
        let line = match item {
            TimelineItem::Date(date) => format!("--- {date} ---"),
            TimelineItem::NewMessages { .. } => "--- new messages ---".to_owned(),
            TimelineItem::Hole => "... (not loaded) ...".to_owned(),
            TimelineItem::UserActivity(posts) => format!("[{} membership messages]", posts.len()),
            TimelineItem::Post { post, consecutive } => {
                let author = store
                    .user_name(&post.user_id)
                    .unwrap_or_else(|| post.user_id.to_string());
                let message: String = post.message.replace('\n', " ⏎ ").chars().take(70).collect();
                let pending = if store.pending.contains_key(post.id.as_str()) {
                    " (sending)"
                } else {
                    ""
                };
                if *consecutive {
                    format!("    {message}{pending}")
                } else {
                    format!("{author}: {message}{pending}")
                }
            }
        };
        println!("    {line}");
    }
}

fn search(engine: &mm_sync::EngineHandle, servers_dir: &std::path::Path, words: &str) {
    let store = engine.store().read().expect("store lock");
    let Some(account) = std::fs::read_dir(servers_dir)
        .ok()
        .and_then(|mut dirs| dirs.next())
        .and_then(|dir| dir.ok())
    else {
        println!("? no cache yet");
        return;
    };
    match mm_cache::open_reader(&account.path().join("cache.sqlite"))
        .map_err(anyhow::Error::from)
        .and_then(|mut reader| Ok(mm_cache::search(&mut reader, words, None, 10)?))
    {
        Ok(hits) => {
            println!("  {} hits for `{words}`:", hits.len());
            for hit in hits {
                let message = store
                    .posts
                    .get(&hit.post)
                    .map(|post| {
                        post.message
                            .replace('\n', " ⏎ ")
                            .chars()
                            .take(70)
                            .collect::<String>()
                    })
                    .unwrap_or_else(|| "(not in memory)".to_owned());
                println!("    {}: {message}", name(&store, &hit.channel));
            }
        }
        Err(error) => println!("? search: {error}"),
    }
}
