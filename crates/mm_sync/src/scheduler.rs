// SPDX-License-Identifier: AGPL-3.0-only

//! The eager sync scheduler (MatterLite.md §8.5): what to fetch, in which order, and the
//! fetching itself. Jobs only fetch; the engine loop applies their results to the store, so
//! the store keeps a single writer and live events never wait for a page load.

use std::collections::{HashSet, VecDeque};

use mm_api::{ApiError, Client, PostsQuery, ThreadsQuery};
use mm_model::{ChannelId, Emoji, Post, PostId, PostList, TeamId, ThreadList, User, UserId};
use mm_store::{PageRequest, Store};

use crate::History;
use crate::bootstrap::{PAGE, SINCE_CAP};
use crate::limiter::Limiter;

/// Backfill pages are as large as the server allows.
pub(crate) const BACKFILL_PAGE: u32 = 200;
/// `POST /users/ids` batches.
pub(crate) const USER_BATCH: usize = 100;
/// `GET /emoji` page size.
pub(crate) const EMOJI_PAGE: u32 = 200;
/// Followed threads per page.
pub(crate) const THREAD_PAGE: u32 = 25;

/// Lower runs first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Priority {
    /// What the user is waiting for (opening a chat, scrolling, opening a thread).
    Interactive = 0,
    /// P1: profiles and custom emoji.
    People = 1,
    /// P2: the newest page of every channel.
    Recent = 2,
    /// P3: followed threads and their replies.
    Threads = 3,
    /// P5: history backfill within the window.
    Backfill = 5,
    /// The `since` sweep that catches edits and deletes in quiet channels.
    Sweep = 6,
}

const PRIORITIES: usize = 7;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Job {
    /// The unread window (`unread`) or the newest page.
    Open {
        channel: ChannelId,
        unread: bool,
    },
    Older {
        channel: ChannelId,
        anchor: PostId,
    },
    Newer {
        channel: ChannelId,
        anchor: PostId,
    },
    /// A post and the pages before and after it.
    Around {
        channel: ChannelId,
        post: PostId,
    },
    Thread(PostId),
    KnownUsers,
    Users(Vec<UserId>),
    /// Profiles changed since then, among the known users.
    ModifiedUsers {
        since: i64,
    },
    Emoji {
        page: u32,
    },
    Newest(ChannelId),
    Threads {
        team: TeamId,
        before: Option<PostId>,
    },
    Backfill {
        channel: ChannelId,
        anchor: PostId,
    },
    Sweep {
        channel: ChannelId,
        since: i64,
    },
}

impl Job {
    /// Jobs with the same key aren't queued twice.
    fn key(&self) -> String {
        match self {
            Self::Open { channel, .. } => format!("open {channel}"),
            Self::Older { channel, .. } => format!("older {channel}"),
            Self::Newer { channel, .. } => format!("newer {channel}"),
            Self::Around { channel, post } => format!("around {channel} {post}"),
            Self::Thread(root) => format!("thread {root}"),
            Self::KnownUsers => "known users".to_owned(),
            Self::Users(ids) => format!("users {}", ids.first().map_or("", |id| id.as_str())),
            Self::ModifiedUsers { .. } => "modified users".to_owned(),
            Self::Emoji { page } => format!("emoji {page}"),
            Self::Newest(channel) => format!("newest {channel}"),
            Self::Threads { team, before } => format!(
                "threads {team} {}",
                before.as_ref().map_or("", |id| id.as_str())
            ),
            Self::Backfill { channel, .. } => format!("backfill {channel}"),
            Self::Sweep { channel, .. } => format!("sweep {channel}"),
        }
    }
}

/// What a job fetched.
#[derive(Debug)]
pub(crate) enum Outcome {
    Page {
        channel: ChannelId,
        list: PostList,
        request: PageRequest,
    },
    Thread {
        root: PostId,
        list: PostList,
    },
    Threads {
        team: TeamId,
        list: ThreadList,
    },
    KnownUsers(Vec<UserId>),
    Users(Vec<User>),
    Emoji {
        page: u32,
        list: Vec<Emoji>,
    },
}

/// The queue: FIFO per priority, no duplicates among queued and running jobs.
#[derive(Debug, Default)]
pub(crate) struct Queue {
    queues: [VecDeque<(Priority, Job)>; PRIORITIES],
    keys: HashSet<String>,
}

impl Queue {
    /// Adds a job unless the same one is queued or running. Returns whether it was added.
    pub(crate) fn push(&mut self, priority: Priority, job: Job) -> bool {
        if !self.keys.insert(job.key()) {
            return false;
        }
        self.queues[priority as usize].push_back((priority, job));
        true
    }

    /// Puts a job back at the front (after a `429` or a network error).
    pub(crate) fn retry(&mut self, priority: Priority, job: Job) {
        self.keys.insert(job.key());
        self.queues[priority as usize].push_front((priority, job));
    }

    /// The next job. Its key stays reserved until [`Queue::finished`].
    pub(crate) fn pop(&mut self) -> Option<(Priority, Job)> {
        self.queues.iter_mut().find_map(VecDeque::pop_front)
    }

    /// The next interactive job only (the eager sync is paused).
    pub(crate) fn pop_interactive(&mut self) -> Option<(Priority, Job)> {
        self.queues[Priority::Interactive as usize].pop_front()
    }

    pub(crate) fn finished(&mut self, job: &Job) {
        self.keys.remove(&job.key());
    }

    pub(crate) fn len(&self) -> usize {
        self.queues.iter().map(VecDeque::len).sum()
    }
}

/// Joined, non-archived channels in the order the eager sync takes them (§8.5): mentions,
/// unread, favourites, DMs/GMs, recently viewed, the rest.
pub(crate) fn channel_order(store: &Store) -> Vec<ChannelId> {
    let favorites: HashSet<&ChannelId> = store
        .categories
        .values()
        .flatten()
        .filter(|category| category.category_type == "favorites")
        .flat_map(|category| &category.channel_ids)
        .collect();
    let mut channels: Vec<_> = store
        .channels
        .values()
        .filter(|channel| !channel.is_deleted() && store.channel_members.contains_key(&channel.id))
        .map(|channel| {
            let unread = store.channel_unread(&channel.id).unwrap_or_default();
            let viewed = store
                .channel_members
                .get(&channel.id)
                .map_or(0, |member| member.last_viewed_at);
            let rank = if unread.mentions > 0 {
                0
            } else if unread.is_unread() {
                1
            } else if favorites.contains(&channel.id) {
                2
            } else if channel.channel_type.is_direct_or_group() {
                3
            } else {
                4
            };
            (rank, std::cmp::Reverse(viewed), channel.id.clone())
        })
        .collect();
    channels.sort();
    channels.into_iter().map(|(_, _, id)| id).collect()
}

/// P2: channels whose newest posts aren't loaded.
pub(crate) fn plan_recent(store: &Store) -> Vec<ChannelId> {
    channel_order(store)
        .into_iter()
        .filter(|channel| {
            store
                .timelines
                .get(channel)
                .is_none_or(|timeline| !timeline.has_recent())
        })
        .collect()
}

/// P5: the next backfill page of a channel, if its loaded history doesn't cover the window
/// yet: the oldest post of the stretch that reaches the newest post, when that stretch
/// doesn't reach the start and is newer than the cutoff.
pub(crate) fn backfill_anchor(
    store: &Store,
    channel: &ChannelId,
    history: History,
    now: i64,
) -> Option<PostId> {
    let cutoff = match history {
        History::Off => return None,
        History::All => i64::MIN,
        History::Days(days) => now - i64::from(days) * 24 * 60 * 60 * 1000,
    };
    let timeline = store.timelines.get(channel)?;
    let recent = timeline.ranges().last().filter(|range| range.recent)?;
    if recent.start {
        return None;
    }
    let (oldest_at, oldest) = timeline.oldest_of_recent()?;
    (*oldest_at > cutoff).then(|| oldest.clone())
}

/// P5 for every channel, in the eager order.
pub(crate) fn plan_backfill(store: &Store, history: History, now: i64) -> Vec<(ChannelId, PostId)> {
    channel_order(store)
        .into_iter()
        .filter_map(|channel| {
            let anchor = backfill_anchor(store, &channel, history, now)?;
            Some((channel, anchor))
        })
        .collect()
}

/// P3: followed threads whose replies aren't all loaded, active within the window.
pub(crate) fn threads_to_load(
    store: &Store,
    list: &ThreadList,
    history: History,
    now: i64,
) -> Vec<PostId> {
    let cutoff = match history {
        History::Off => return Vec::new(),
        History::All => i64::MIN,
        History::Days(days) => now - i64::from(days) * 24 * 60 * 60 * 1000,
    };
    list.threads
        .iter()
        .filter(|thread| thread.last_reply_at >= cutoff)
        .filter(|thread| {
            store.threads.get(&thread.id).is_none_or(|replies| {
                !replies.is_complete()
                    || replies
                        .newest()
                        .is_none_or(|(at, _)| *at < thread.last_reply_at)
            })
        })
        .map(|thread| thread.id.clone())
        .collect()
}

/// What the jobs need to run.
#[derive(Clone)]
pub(crate) struct Context {
    pub client: Client,
    pub limiter: Limiter,
    pub me: UserId,
    pub crt: bool,
}

impl Context {
    fn posts(&self) -> PostsQuery {
        PostsQuery {
            collapsed_threads: self.crt,
            collapsed_threads_extended: self.crt,
            ..PostsQuery::default()
        }
    }
}

/// Runs one job: every request waits for the rate limiter first.
pub(crate) async fn run(context: Context, job: Job) -> Result<Outcome, ApiError> {
    let client = &context.client;
    context.limiter.acquire().await;
    Ok(match job {
        Job::Open { channel, unread } => {
            let (list, request) = if unread {
                let list = client
                    .posts_around_first_unread(&context.me, &channel, 30, 30, context.crt)
                    .await?;
                (list, PageRequest::AroundUnread)
            } else {
                let query = PostsQuery {
                    per_page: Some(PAGE),
                    ..context.posts()
                };
                (
                    client.channel_posts(&channel, &query).await?,
                    PageRequest::Newest,
                )
            };
            Outcome::Page {
                channel,
                list,
                request,
            }
        }
        Job::Newest(channel) => {
            let query = PostsQuery {
                per_page: Some(PAGE),
                ..context.posts()
            };
            Outcome::Page {
                list: client.channel_posts(&channel, &query).await?,
                channel,
                request: PageRequest::Newest,
            }
        }
        Job::Older { channel, anchor } => before(client, &context, channel, anchor, PAGE).await?,
        Job::Around { channel, post } => {
            let target = client.get_post(&post).await?;
            let page = |before: bool| PostsQuery {
                before: before.then(|| post.clone()),
                after: (!before).then(|| post.clone()),
                per_page: Some(AROUND_PAGE),
                ..context.posts()
            };
            context.limiter.acquire().await;
            let older = client.channel_posts(&channel, &page(true)).await?;
            context.limiter.acquire().await;
            let newer = client.channel_posts(&channel, &page(false)).await?;
            Outcome::Page {
                list: around_list(target, older, newer),
                channel,
                request: PageRequest::Around,
            }
        }
        Job::Backfill { channel, anchor } => {
            before(client, &context, channel, anchor, BACKFILL_PAGE).await?
        }
        Job::Newer { channel, anchor } => {
            let query = PostsQuery {
                after: Some(anchor.clone()),
                per_page: Some(PAGE),
                ..context.posts()
            };
            Outcome::Page {
                list: client.channel_posts(&channel, &query).await?,
                channel,
                request: PageRequest::After(anchor),
            }
        }
        Job::Sweep { channel, since } => {
            let query = PostsQuery {
                since: Some(since),
                ..context.posts()
            };
            let list = client.channel_posts(&channel, &query).await?;
            let complete = list.order.len() < SINCE_CAP;
            Outcome::Page {
                channel,
                list,
                request: PageRequest::Since { complete },
            }
        }
        Job::Thread(root) => Outcome::Thread {
            list: client.post_thread(&root).await?,
            root,
        },
        Job::Threads { team, before } => {
            let query = ThreadsQuery {
                before,
                per_page: Some(THREAD_PAGE),
                ..ThreadsQuery::default()
            };
            Outcome::Threads {
                list: client.team_threads(&context.me, &team, &query).await?,
                team,
            }
        }
        Job::KnownUsers => Outcome::KnownUsers(client.known_users().await?),
        Job::Users(ids) => Outcome::Users(client.users_by_ids(&ids).await?),
        Job::ModifiedUsers { since } => {
            let known = client.known_users().await?;
            let mut users = Vec::new();
            for batch in known.chunks(USER_BATCH) {
                context.limiter.acquire().await;
                users.extend(client.users_by_ids_since(batch, since).await?);
            }
            Outcome::Users(users)
        }
        Job::Emoji { page } => Outcome::Emoji {
            list: client.custom_emoji(page, EMOJI_PAGE).await?,
            page,
        },
    })
}

/// Posts on each side of a post shown from elsewhere.
const AROUND_PAGE: u32 = 30;

/// One page of a post with the posts older and newer than it: newest first, from the older
/// page's previous post to the newer page's next one.
pub(crate) fn around_list(target: Post, older: PostList, newer: PostList) -> PostList {
    let mut posts = older.posts;
    posts.extend(newer.posts);
    let mut order: Vec<PostId> = newer.order;
    order.push(target.id.clone());
    order.extend(older.order);
    posts.insert(target.id.clone(), target);
    order.sort_by_key(|id| std::cmp::Reverse(posts.get(id).map_or(0, |post| post.create_at)));
    order.dedup();
    PostList {
        order,
        posts,
        next_post_id: newer.next_post_id,
        prev_post_id: older.prev_post_id,
        ..PostList::default()
    }
}

async fn before(
    client: &Client,
    context: &Context,
    channel: ChannelId,
    anchor: PostId,
    per_page: u32,
) -> Result<Outcome, ApiError> {
    let query = PostsQuery {
        before: Some(anchor.clone()),
        per_page: Some(per_page),
        ..context.posts()
    };
    Ok(Outcome::Page {
        list: client.channel_posts(&channel, &query).await?,
        channel,
        request: PageRequest::Before(anchor),
    })
}

#[cfg(test)]
#[path = "scheduler_tests.rs"]
mod tests;
