// SPDX-License-Identifier: AGPL-3.0-only

//! `mm-cli`: tries the protocol crates (`mm_api`, `mm_ws`) against a real server.
//!
//! ```text
//! mm-cli <server-url> ping
//! mm-cli <server-url> bootstrap [--login <user>]
//! mm-cli <server-url> events [--login <user>]
//! ```
//!
//! Authentication: `MM_TOKEN` (a session or personal access token), or a password login with
//! `--login` (asked for when missing), the password from `MM_PASSWORD` or asked for without
//! echo, and the MFA code asked for when the server wants one. A session this tool logged in is
//! logged out when it exits. The token is never printed.

use std::collections::HashMap;
use std::io::{self, IsTerminal as _, Write as _};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, bail};
use mm_api::{Client, ClientOptions, LoginRequest, ServerUrl};
use mm_model::preference::keys;
use mm_model::ws::{Broadcast, WsEvent};
use mm_model::{
    Channel, ChannelId, ChannelMember, ChannelType, SessionToken, Status, User, UserId,
};
use mm_ws::{WsConfig, WsOutput};

const USAGE: &str = "usage: mm-cli <server-url> <ping|bootstrap|events> [--login <user>]";

enum Command {
    Ping,
    LoggedIn(Task),
}

#[derive(Clone, Copy)]
enum Task {
    Bootstrap,
    Events,
}

struct Args {
    server: ServerUrl,
    command: Command,
    login: Option<String>,
}

fn parse_args() -> Result<Args> {
    let mut args = std::env::args().skip(1);
    let mut positional = Vec::new();
    let mut login = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--login" => login = Some(args.next().context("--login needs a user name")?),
            "-h" | "--help" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            _ => positional.push(arg),
        }
    }
    let [server, command] = positional.as_slice() else {
        bail!(USAGE);
    };
    let command = match command.as_str() {
        "ping" => Command::Ping,
        "bootstrap" => Command::LoggedIn(Task::Bootstrap),
        "events" => Command::LoggedIn(Task::Events),
        other => bail!("unknown command `{other}`\n{USAGE}"),
    };
    Ok(Args {
        server: ServerUrl::parse(server)?,
        command,
        login,
    })
}

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    let args = parse_args()?;
    let client = Client::new(args.server, ClientOptions::default())?;
    let task = match args.command {
        Command::Ping => return ping(&client).await,
        Command::LoggedIn(task) => task,
    };
    let (client, logged_in_here) = authenticate(client, args.login).await?;
    let result = match task {
        Task::Bootstrap => bootstrap(&client).await,
        Task::Events => events(&client).await,
    };
    if logged_in_here {
        match client.logout().await {
            Ok(()) => println!("logged out"),
            Err(error) => eprintln!("logout failed: {error}"),
        }
    }
    result
}

fn prompt(question: &str) -> Result<String> {
    print!("{question}");
    io::stdout().flush()?;
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    Ok(answer.trim().to_owned())
}

/// Returns the client with a token and whether this tool created the session.
async fn authenticate(mut client: Client, login: Option<String>) -> Result<(Client, bool)> {
    if let Ok(token) = std::env::var("MM_TOKEN")
        && !token.is_empty()
    {
        client.set_token(Some(SessionToken::new(token)));
        return Ok((client, false));
    }
    let has_password = std::env::var("MM_PASSWORD").is_ok_and(|password| !password.is_empty());
    if !io::stdin().is_terminal() && (login.is_none() || !has_password) {
        bail!("no terminal to ask for credentials: set MM_TOKEN, or MM_PASSWORD with --login");
    }
    let login_id = match login {
        Some(login) => login,
        None => prompt("login (username or email): ")?,
    };
    let password = match std::env::var("MM_PASSWORD") {
        Ok(password) if !password.is_empty() => password,
        _ => rpassword::prompt_password("password: ")?,
    };
    let mut request = LoginRequest {
        login_id,
        password,
        ..LoginRequest::default()
    };
    let response = match client.login(&request).await {
        Err(error) if error.needs_mfa_code() => {
            request.mfa_code = prompt("MFA code: ")?;
            client.login(&request).await?
        }
        other => other?,
    };
    println!("logged in as @{}", response.user.username);
    client.set_token(Some(response.token));
    Ok((client, true))
}

async fn ping(client: &Client) -> Result<()> {
    let start = Instant::now();
    client.ping().await?;
    println!("ping OK in {} ms", start.elapsed().as_millis());
    let config = client.client_config().await?;
    println!(
        "server     {}",
        client.server_version().as_deref().unwrap_or("?")
    );
    println!("site URL   {}", config.site_url().unwrap_or("(not set)"));
    if let Some(url) = config.websocket_url() {
        println!("WebSocket  {url}");
    }
    let sign_in: Vec<&str> = [
        ("EnableSignInWithEmail", "email"),
        ("EnableSignInWithUsername", "username"),
        ("EnableLdap", "LDAP"),
        ("EnableSaml", "SAML"),
        ("EnableSignUpWithGitLab", "GitLab"),
        ("EnableSignUpWithGoogle", "Google"),
        ("EnableSignUpWithOffice365", "Office 365"),
        ("EnableSignUpWithOpenId", "OpenID"),
    ]
    .into_iter()
    .filter(|(key, _)| config.bool(key))
    .map(|(_, name)| name)
    .collect();
    println!("sign-in    {}", sign_in.join(", "));
    Ok(())
}

/// The startup sequence of WebApp.md §6, with a summary of what came back.
async fn bootstrap(client: &Client) -> Result<()> {
    let mut timings = Vec::new();
    let mut step = Instant::now();
    let mut lap = |name: &'static str, step: &mut Instant| {
        timings.push((name, step.elapsed()));
        *step = Instant::now();
    };

    let (config, license) = tokio::try_join!(client.client_config(), client.client_license())?;
    lap("config + license", &mut step);
    let (me, preferences, teams, team_members) = tokio::try_join!(
        client.me(),
        client.my_preferences(),
        client.my_teams(),
        client.my_team_members(),
    )?;
    lap("me, preferences, teams, members", &mut step);
    let mut role_names: Vec<&str> = me
        .role_names()
        .chain(
            team_members
                .iter()
                .flat_map(|member| member.roles.split_whitespace()),
        )
        .collect();
    role_names.sort_unstable();
    role_names.dedup();
    let roles = client.roles_by_names(&role_names).await?;
    lap("roles", &mut step);

    let crt_preference = preferences
        .iter()
        .find(|p| p.category == keys::DISPLAY_SETTINGS && p.name == keys::COLLAPSED_REPLY_THREADS)
        .map(|p| p.value.as_str());
    let crt_setting = config.collapsed_threads();
    let crt = crt_setting.is_enabled(crt_preference);

    let (channels, members) =
        tokio::try_join!(client.my_channels(), client.my_channel_members(&me.id))?;
    lap("channels + memberships", &mut step);
    let unreads = client.my_team_unreads(crt).await?;
    lap("team unreads", &mut step);
    let mut categories = Vec::new();
    for team in &teams {
        categories.push((team, client.sidebar_categories(&me.id, &team.id).await?));
    }
    lap("sidebar categories", &mut step);

    let mut partner_ids: Vec<UserId> = channels
        .iter()
        .filter(|channel| channel.channel_type == ChannelType::Direct)
        .filter_map(|channel| dm_partner(&channel.name, &me.id))
        .collect();
    partner_ids.sort_unstable();
    partner_ids.dedup();
    let (profiles, statuses) = if partner_ids.is_empty() {
        (Vec::new(), Vec::new())
    } else {
        tokio::try_join!(
            client.users_by_ids(&partner_ids),
            client.statuses_by_ids(&partner_ids)
        )?
    };
    lap("DM profiles + statuses", &mut step);

    println!(
        "server     {} (licensed: {})",
        client.server_version().as_deref().unwrap_or("?"),
        if license.bool("IsLicensed") {
            "yes"
        } else {
            "no"
        }
    );
    println!(
        "user       @{} {}  roles: {}",
        me.username,
        full_name(&me),
        me.roles
    );
    println!(
        "CRT        server {crt_setting:?}, preference {}: {}",
        crt_preference.unwrap_or("(not set)"),
        if crt { "on" } else { "off" }
    );
    println!(
        "roles      {} with {} permissions",
        roles.len(),
        roles
            .iter()
            .map(|role| role.permissions.len())
            .sum::<usize>()
    );

    println!("\nteams ({})", teams.len());
    for team in &teams {
        let unread = unreads.iter().find(|unread| unread.team_id == team.id);
        let (messages, mentions) = unread.map_or((0, 0), |unread| {
            if crt {
                (unread.msg_count_root, unread.mention_count_root)
            } else {
                (unread.msg_count, unread.mention_count)
            }
        });
        println!(
            "  {:<24} {:<28} {messages} unread, {mentions} mentions{}",
            team.name,
            team.display_name,
            unread
                .filter(|_| crt)
                .map(|unread| format!(
                    "; threads {} unread, {} mentions",
                    unread.thread_count, unread.thread_mention_count
                ))
                .unwrap_or_default()
        );
    }

    let members: HashMap<&ChannelId, &ChannelMember> = members
        .iter()
        .map(|member| (&member.channel_id, member))
        .collect();
    let profiles: HashMap<&UserId, &User> = profiles.iter().map(|user| (&user.id, user)).collect();
    let statuses: HashMap<&UserId, &Status> = statuses
        .iter()
        .map(|status| (&status.user_id, status))
        .collect();
    println!(
        "\nchannels ({}, {} memberships)",
        channels.len(),
        members.len()
    );
    let mut sections: Vec<(String, Vec<&Channel>)> = teams
        .iter()
        .map(|team| {
            let in_team = channels.iter().filter(|c| c.team_id == team.id).collect();
            (team.display_name.clone(), in_team)
        })
        .collect();
    sections.push((
        "Direct and group messages".to_owned(),
        channels.iter().filter(|c| c.team_id.is_empty()).collect(),
    ));
    for (title, mut section) in sections {
        let label = |channel: &Channel| match channel.channel_type {
            ChannelType::Direct => match dm_partner(&channel.name, &me.id) {
                Some(partner) => {
                    let name = profiles
                        .get(&partner)
                        .map_or(partner.to_string(), |user| user.username.clone());
                    let status = statuses
                        .get(&partner)
                        .map_or("", |status| status.status.as_str());
                    format!("@{name} ({status})")
                }
                None => channel.name.clone(),
            },
            ChannelType::Group => channel.display_name.clone(),
            ChannelType::Open | ChannelType::Private | ChannelType::Other(_) => {
                format!("~{}", channel.name)
            }
        };
        section.sort_by_key(|channel| label(channel).to_lowercase());
        println!("  [{title}]");
        for channel in section {
            let Some(member) = members.get(&channel.id) else {
                println!("    {:<32} (no membership)", label(channel));
                continue;
            };
            let (unread, mentions) = if crt {
                (
                    channel.total_msg_count_root - member.msg_count_root,
                    member.mention_count_root,
                )
            } else {
                (
                    channel.total_msg_count - member.msg_count,
                    member.mention_count,
                )
            };
            let mut notes = Vec::new();
            if unread > 0 {
                notes.push(format!("{unread} unread"));
            }
            if mentions > 0 {
                notes.push(format!("{mentions} mentions"));
            }
            if member.is_muted() {
                notes.push("muted".to_owned());
            }
            if channel.is_deleted() {
                notes.push("archived".to_owned());
            }
            println!("    {:<32} {}", label(channel), notes.join(", "));
        }
    }

    println!("\nsidebar categories");
    for (team, ordered) in &categories {
        let names: Vec<String> = ordered
            .order
            .iter()
            .filter_map(|id| ordered.categories.iter().find(|c| &c.id == id))
            .map(|category| format!("{} ({})", category.display_name, category.channel_ids.len()))
            .collect();
        println!("  [{}] {}", team.display_name, names.join(" · "));
    }

    println!("\ntimings");
    for (name, elapsed) in timings {
        println!("  {name:<34} {:>6} ms", elapsed.as_millis());
    }
    Ok(())
}

/// The other user of a DM channel named `<id>__<id>` (yourself for a self-DM).
fn dm_partner(channel_name: &str, me: &UserId) -> Option<UserId> {
    let (first, second) = channel_name.split_once("__")?;
    Some(if first == me.as_str() { second } else { first }.into())
}

fn full_name(user: &User) -> String {
    [user.first_name.as_str(), user.last_name.as_str()]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Connects the WebSocket and prints what arrives until Ctrl+C.
async fn events(client: &Client) -> Result<()> {
    let (config, me, channels) =
        tokio::try_join!(client.client_config(), client.me(), client.my_channels())?;
    let names: HashMap<ChannelId, String> = channels
        .into_iter()
        .map(|channel| {
            let name = if channel.channel_type.is_direct_or_group() {
                channel.display_name.clone()
            } else {
                format!("~{}", channel.name)
            };
            (channel.id, name)
        })
        .collect();
    let url = client.server().websocket_url(config.websocket_url())?;
    let token = client.token().context("not logged in")?.clone();
    let (handle, output) = mm_ws::connect(WsConfig::new(
        url.clone(),
        token,
        mm_api::default_user_agent(),
    ));
    println!("connecting to {url} as @{} (Ctrl+C to stop)", me.username);

    let started = Instant::now();
    loop {
        let out = tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                handle.close();
                continue;
            }
            out = output.recv() => match out {
                Ok(out) => out,
                Err(_) => break,
            },
        };
        let time = format_elapsed(started.elapsed());
        match out {
            WsOutput::Connected { resuming } => {
                println!(
                    "{time} connected{}",
                    if resuming { " (resuming)" } else { "" }
                );
            }
            WsOutput::MissedEvents => println!(
                "{time} the server started a new connection: events were missed (a client resyncs now)"
            ),
            WsOutput::Event {
                seq,
                broadcast,
                event,
            } => println!("{time} #{seq:<4} {}", describe(&event, &broadcast, &names)),
            WsOutput::Undecodable { seq, error } => {
                println!("{time} #{seq:<4} undecodable: {error}");
            }
            WsOutput::Response(response) => println!(
                "{time} reply to request {}: {}",
                response.seq_reply, response.status
            ),
            WsOutput::Disconnected {
                reason,
                retry_in,
                session_confirmed,
            } => println!(
                "{time} disconnected: {reason:?}{}; retrying in {:.1} s",
                if session_confirmed {
                    ""
                } else {
                    " before the server confirmed the session (is the token valid?)"
                },
                retry_in.as_secs_f64()
            ),
            WsOutput::AuthFailed => {
                println!("{time} the server rejected the token");
                break;
            }
            WsOutput::Closed => {
                println!("{time} closed");
                break;
            }
        }
    }
    Ok(())
}

fn format_elapsed(elapsed: Duration) -> String {
    format!("[{:>7.1}s]", elapsed.as_secs_f64())
}

fn describe(event: &WsEvent, broadcast: &Broadcast, names: &HashMap<ChannelId, String>) -> String {
    let channel = |id: &ChannelId| names.get(id).cloned().unwrap_or_else(|| id.to_string());
    match event {
        WsEvent::Hello(hello) => format!(
            "hello: connection {} on {}, server {}",
            hello.connection_id, hello.server_hostname, hello.server_version
        ),
        WsEvent::Posted(posted) => format!(
            "posted in {} by {}: {}{}",
            channel(&posted.post.channel_id),
            posted.sender_name,
            shorten(&posted.post.message, 100),
            if posted.post.is_root() {
                ""
            } else {
                " (reply)"
            }
        ),
        WsEvent::PostEdited(post) => format!(
            "edited in {}: {}",
            channel(&post.channel_id),
            shorten(&post.message, 100)
        ),
        WsEvent::PostDeleted(deleted) => format!(
            "deleted in {}: post {}",
            channel(&deleted.post.channel_id),
            deleted.post.id
        ),
        WsEvent::Typing(typing) => format!(
            "typing in {}: {}",
            channel(&broadcast.channel_id),
            typing.user_id
        ),
        WsEvent::StatusChange(change) => format!("status of {}: {}", change.user_id, change.status),
        WsEvent::ReactionAdded(reaction) => format!(
            "reaction :{}: on {} by {}",
            reaction.emoji_name, reaction.post_id, reaction.user_id
        ),
        WsEvent::ReactionRemoved(reaction) => format!(
            "reaction :{}: removed from {}",
            reaction.emoji_name, reaction.post_id
        ),
        WsEvent::Unknown(name) => format!("{name} (not decoded)"),
        other => shorten(&format!("{other:?}"), 160),
    }
}

fn shorten(text: &str, max_chars: usize) -> String {
    let single_line = text.replace('\n', " ⏎ ");
    if single_line.chars().count() <= max_chars {
        single_line
    } else {
        let cut: String = single_line.chars().take(max_chars).collect();
        format!("{cut}…")
    }
}
