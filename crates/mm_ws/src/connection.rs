// SPDX-License-Identifier: AGPL-3.0-only

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_channel::{Receiver, Sender};
use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use mm_model::SessionToken;
use mm_model::error::ids;
use mm_model::ws::{Broadcast, ClientAction, ServerFrame, WsEvent, WsResponse};
use tokio::net::TcpStream;
use tokio::time::{Instant, MissedTickBehavior, interval_at, sleep_until, timeout};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::{HeaderValue, StatusCode, header};
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::{Error as WsError, Message};
use tokio_tungstenite::{
    Connector, MaybeTlsStream, WebSocketStream, connect_async_tls_with_config,
};
use url::Url;

use crate::backoff::Backoff;
use crate::session::{Check, ResumeState, Session};
use crate::tls;

/// Close codes the server understands as the reason for a reconnect (`disconnect_err_code`).
const NORMAL_CLOSE_CODE: u16 = 1000;
const PING_TIMEOUT_CODE: u16 = 4000;
const SEQUENCE_GAP_CODE: u16 = 4001;

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

#[derive(Clone, Debug)]
pub struct WsConfig {
    /// `ws(s)://…/api/v4/websocket` (`mm_api::ServerUrl::websocket_url`).
    pub url: Url,
    pub token: SessionToken,
    pub user_agent: String,
    pub ping_interval: Duration,
    pub connect_timeout: Duration,
    pub backoff: Backoff,
    /// Resume an earlier connection, for example one from before the app restarted.
    pub resume: Option<ResumeState>,
}

impl WsConfig {
    pub fn new(url: Url, token: SessionToken, user_agent: String) -> Self {
        Self {
            url,
            token,
            user_agent,
            ping_interval: Duration::from_secs(30),
            connect_timeout: Duration::from_secs(15),
            backoff: Backoff::default(),
            resume: None,
        }
    }
}

/// What the connection task reports, in order.
#[derive(Debug)]
pub enum WsOutput {
    /// The socket is open. `resuming`: the server was asked to continue an earlier connection;
    /// if it can't, a `hello` with a new connection id follows (after [`WsOutput::MissedEvents`]).
    Connected { resuming: bool },
    /// The server confirmed this connection's session (a ping reply or an event), once per
    /// connection. `resumed`: it continued the earlier connection with nothing lost (no
    /// [`WsOutput::MissedEvents`]), so events since then are replayed and no resync is needed.
    Confirmed { resumed: bool },
    /// The server could not resume: events since the last connection were lost. Run a full
    /// resync (WebApp.md §7.4). Sent before the new connection's `hello`.
    MissedEvents,
    /// An event, in sequence. The channel or team it concerns is usually in `broadcast`.
    Event {
        seq: i64,
        broadcast: Broadcast,
        event: WsEvent,
    },
    /// An event in sequence whose payload could not be decoded.
    Undecodable { seq: i64, error: String },
    /// The reply to a request sent with [`WsHandle::send`].
    Response(WsResponse),
    Disconnected {
        reason: DisconnectReason,
        retry_in: Duration,
        /// Whether the server had confirmed the session (a ping reply or an event) before the
        /// connection ended. The server accepts the upgrade even with an invalid token and then
        /// drops the socket, so repeated unconfirmed disconnects usually mean the token is no
        /// longer valid: check it with a REST call (401: log in again). Unconfirmed connections
        /// count as failures for the backoff.
        session_confirmed: bool,
    },
    /// The server rejected the token: a 401 on the upgrade request, or a request on an open
    /// socket answered with `api.web_socket_router.not_authenticated.app_error` (the session
    /// was revoked or expired; the server keeps such sockets open but sends no more events).
    /// The task has ended; log in again.
    AuthFailed,
    /// [`WsHandle::close`] was called or the handle dropped. The task has ended.
    Closed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DisconnectReason {
    ConnectFailed(String),
    /// A ping went unanswered for a whole ping interval.
    PingTimeout,
    /// An event was missing; the next connection asks the server to replay from `expected`.
    SequenceGap {
        expected: i64,
        got: i64,
    },
    ServerClosed {
        code: Option<u16>,
        reason: String,
    },
    Error(String),
    /// [`WsHandle::reconnect_now`].
    ReconnectRequested,
}

impl DisconnectReason {
    fn close_code(&self) -> Option<u16> {
        match self {
            Self::PingTimeout => Some(PING_TIMEOUT_CODE),
            Self::SequenceGap { .. } => Some(SEQUENCE_GAP_CODE),
            Self::ServerClosed { code, .. } => *code,
            Self::ReconnectRequested => Some(NORMAL_CLOSE_CODE),
            Self::ConnectFailed(_) | Self::Error(_) => None,
        }
    }
}

enum Command {
    Send(ClientAction),
    ReconnectNow,
    Close,
}

/// Controls the connection task. Clones share the task; it ends when the last one is dropped.
#[derive(Clone, Debug)]
pub struct WsHandle {
    commands: Sender<Command>,
    resume: Arc<Mutex<ResumeState>>,
}

impl WsHandle {
    /// Sends a request (typing, presence, statuses). Requests made while disconnected are
    /// dropped: they only matter for the moment. Returns `false` when the task has ended.
    pub fn send(&self, action: ClientAction) -> bool {
        self.commands.try_send(Command::Send(action)).is_ok()
    }

    /// Reconnects at once without backoff, e.g. after the OS woke from sleep or the network
    /// changed.
    pub fn reconnect_now(&self) {
        // Fails only when the task has ended, and then there is nothing to reconnect.
        let _ = self.commands.try_send(Command::ReconnectNow);
    }

    pub fn close(&self) {
        let _ = self.commands.try_send(Command::Close);
    }

    /// Where the event stream stands: save it (on shutdown and now and then) and pass it as
    /// [`WsConfig::resume`] after a restart to get the missed events replayed.
    pub fn resume_state(&self) -> ResumeState {
        self.resume
            .lock()
            .map(|state| state.clone())
            .unwrap_or_default()
    }
}

/// Starts the connection task on the current Tokio runtime.
pub fn connect(config: WsConfig) -> (WsHandle, Receiver<WsOutput>) {
    let (command_tx, command_rx) = async_channel::unbounded();
    let (output_tx, output_rx) = async_channel::unbounded();
    let resume = Arc::new(Mutex::new(config.resume.clone().unwrap_or_default()));
    tokio::spawn(run(config, command_rx, output_tx, resume.clone()));
    (
        WsHandle {
            commands: command_tx,
            resume,
        },
        output_rx,
    )
}

enum Ended {
    Closed,
    OutputDropped,
    AuthFailed,
    Disconnected(DisconnectReason),
}

async fn run(
    config: WsConfig,
    commands: Receiver<Command>,
    output: Sender<WsOutput>,
    resume: Arc<Mutex<ResumeState>>,
) {
    let mut session = Session::new(config.resume.clone());
    let mut fail_count = 0;
    let mut last_close_code = None;
    loop {
        let url = session.connect_url(&config.url, last_close_code);
        let resuming = session.is_resuming();
        let mut session_confirmed = false;
        let reason = match open(&config, &url).await {
            Ok(socket) => {
                if output.send(WsOutput::Connected { resuming }).await.is_err() {
                    return;
                }
                let ended = drive(
                    &config,
                    socket,
                    Stream {
                        session: &mut session,
                        resuming,
                        resume: &resume,
                    },
                    &commands,
                    &output,
                    &mut session_confirmed,
                )
                .await;
                if session_confirmed {
                    fail_count = 0;
                }
                match ended {
                    Ended::Closed => {
                        let _ = output.send(WsOutput::Closed).await;
                        return;
                    }
                    Ended::OutputDropped => return,
                    Ended::AuthFailed => {
                        let _ = output.send(WsOutput::AuthFailed).await;
                        return;
                    }
                    Ended::Disconnected(reason) => reason,
                }
            }
            Err(OpenError::Unauthorized) => {
                let _ = output.send(WsOutput::AuthFailed).await;
                return;
            }
            Err(OpenError::Failed(message)) => DisconnectReason::ConnectFailed(message),
        };

        last_close_code = reason.close_code();
        let delay = if reason == DisconnectReason::ReconnectRequested {
            Duration::ZERO
        } else {
            fail_count += 1;
            config.backoff.delay(fail_count, rand::random())
        };
        log::info!("WebSocket disconnected ({reason:?}); reconnecting in {delay:?}");
        if output
            .send(WsOutput::Disconnected {
                reason,
                retry_in: delay,
                session_confirmed,
            })
            .await
            .is_err()
        {
            return;
        }

        let deadline = Instant::now() + delay;
        loop {
            tokio::select! {
                () = sleep_until(deadline) => break,
                command = commands.recv() => match command {
                    Ok(Command::ReconnectNow) => {
                        fail_count = 0;
                        break;
                    }
                    Ok(Command::Send(action)) => {
                        log::debug!("dropping `{}`: not connected", action.name());
                    }
                    Ok(Command::Close) | Err(_) => {
                        let _ = output.send(WsOutput::Closed).await;
                        return;
                    }
                },
            }
        }
    }
}

enum OpenError {
    Unauthorized,
    Failed(String),
}

async fn open(config: &WsConfig, url: &Url) -> Result<Socket, OpenError> {
    let failed = |error: &dyn std::fmt::Display| OpenError::Failed(error.to_string());
    let mut request = url
        .as_str()
        .into_client_request()
        .map_err(|error| failed(&error))?;
    let mut authorization = HeaderValue::from_str(&format!("Bearer {}", config.token.expose()))
        .map_err(|error| failed(&error))?;
    authorization.set_sensitive(true);
    let headers = request.headers_mut();
    headers.insert(header::AUTHORIZATION, authorization);
    if let Ok(user_agent) = HeaderValue::from_str(&config.user_agent) {
        headers.insert(header::USER_AGENT, user_agent);
    }
    let connector = match url.scheme() {
        "wss" => Connector::Rustls(tls::client_config().map_err(|error| failed(&error))?),
        _ => Connector::Plain,
    };
    let connecting = connect_async_tls_with_config(request, None, true, Some(connector));
    match timeout(config.connect_timeout, connecting).await {
        Err(_) => Err(OpenError::Failed("timed out".to_owned())),
        Ok(Ok((socket, _))) => Ok(socket),
        Ok(Err(WsError::Http(response))) if response.status() == StatusCode::UNAUTHORIZED => {
            Err(OpenError::Unauthorized)
        }
        Ok(Err(error)) => Err(failed(&error)),
    }
}

/// Client request sequence numbers; they start at 1 on every connection.
struct Requests {
    next_seq: i64,
}

impl Requests {
    fn frame(&mut self, action: &ClientAction) -> (i64, Message) {
        let seq = self.next_seq;
        self.next_seq += 1;
        (seq, Message::Text(action.to_frame(seq).into()))
    }
}

/// The event stream state `drive` updates.
struct Stream<'a> {
    session: &'a mut Session,
    /// This connection asked to resume an earlier one.
    resuming: bool,
    /// Shared with [`WsHandle::resume_state`].
    resume: &'a Mutex<ResumeState>,
}

async fn drive(
    config: &WsConfig,
    socket: Socket,
    events: Stream<'_>,
    commands: &Receiver<Command>,
    output: &Sender<WsOutput>,
    session_confirmed: &mut bool,
) -> Ended {
    let Stream {
        session,
        resuming,
        resume,
    } = events;
    let mut missed = false;
    let (mut sink, mut stream) = socket.split();
    let mut requests = Requests { next_seq: 1 };

    // A connection authenticated by the header ignores the challenge; it is sent anyway in
    // case a proxy dropped the header.
    let (auth_seq, challenge) =
        requests.frame(&ClientAction::AuthenticationChallenge(config.token.clone()));
    // Ping at once to test the socket, then every interval (`websocket.ts` `onopen`).
    let (ping_seq, ping) = requests.frame(&ClientAction::Ping);
    for message in [challenge, ping] {
        if let Err(error) = sink.send(message).await {
            return Ended::Disconnected(DisconnectReason::Error(error.to_string()));
        }
    }
    let mut pending_ping = Some(ping_seq);
    let mut ticker = interval_at(Instant::now() + config.ping_interval, config.ping_interval);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            message = stream.next() => {
                let message = match message {
                    None => {
                        return Ended::Disconnected(DisconnectReason::ServerClosed {
                            code: None,
                            reason: "connection ended".to_owned(),
                        });
                    }
                    Some(Err(error)) => {
                        return Ended::Disconnected(DisconnectReason::Error(error.to_string()));
                    }
                    Some(Ok(message)) => message,
                };
                let text = match message {
                    Message::Text(text) => text,
                    Message::Close(frame) => {
                        return Ended::Disconnected(DisconnectReason::ServerClosed {
                            code: frame.as_ref().map(|frame| u16::from(frame.code)),
                            reason: frame.map(|frame| frame.reason.to_string()).unwrap_or_default(),
                        });
                    }
                    // Pings are answered by tungstenite; binary (msgpack) frames are never
                    // requested.
                    Message::Binary(_) | Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => {
                        continue;
                    }
                };
                // `RUST_LOG=mm_ws::frames=trace` logs every frame as received.
                log::trace!(target: "mm_ws::frames", "{}", text.as_str());
                let frame = match ServerFrame::parse(text.as_str()) {
                    Ok(frame) => frame,
                    Err(error) => {
                        log::warn!("ignoring a malformed WebSocket frame: {error}");
                        continue;
                    }
                };
                let out = match frame {
                    ServerFrame::Response(response) => {
                        if response
                            .error
                            .as_ref()
                            .is_some_and(|error| error.id == ids::WS_NOT_AUTHENTICATED)
                        {
                            close(&mut sink, &mut stream, NORMAL_CLOSE_CODE).await;
                            return Ended::AuthFailed;
                        }
                        if pending_ping == Some(response.seq_reply) {
                            log::debug!("ping reply: {} {:?}", response.status, response.error);
                            pending_ping = None;
                            if response.is_ok() && !*session_confirmed {
                                *session_confirmed = true;
                                let resumed = resuming && !missed;
                                if output.send(WsOutput::Confirmed { resumed }).await.is_err() {
                                    return Ended::OutputDropped;
                                }
                            }
                            continue;
                        }
                        if response.seq_reply == auth_seq {
                            if !response.is_ok() {
                                log::warn!("WebSocket authentication challenge failed: {:?}", response.error);
                            }
                            continue;
                        }
                        WsOutput::Response(response)
                    }
                    ServerFrame::Event(frame) => match session.check(&frame) {
                        Check::Gap { expected, got } => {
                            close(&mut sink, &mut stream, SEQUENCE_GAP_CODE).await;
                            return Ended::Disconnected(DisconnectReason::SequenceGap { expected, got });
                        }
                        Check::Accepted { missed_events } => {
                            if let Ok(mut state) = resume.lock() {
                                *state = session.state().clone();
                            }
                            if missed_events {
                                missed = true;
                                if output.send(WsOutput::MissedEvents).await.is_err() {
                                    return Ended::OutputDropped;
                                }
                            }
                            if !*session_confirmed {
                                *session_confirmed = true;
                                let resumed = resuming && !missed;
                                if output.send(WsOutput::Confirmed { resumed }).await.is_err() {
                                    return Ended::OutputDropped;
                                }
                            }
                            match WsEvent::decode(&frame) {
                                Ok(event) => WsOutput::Event {
                                    seq: frame.seq,
                                    broadcast: frame.broadcast,
                                    event,
                                },
                                Err(error) => {
                                    log::warn!("{error}");
                                    WsOutput::Undecodable { seq: frame.seq, error: error.to_string() }
                                }
                            }
                        }
                    },
                };
                if output.send(out).await.is_err() {
                    return Ended::OutputDropped;
                }
            }
            _ = ticker.tick() => {
                if pending_ping.is_some() {
                    close(&mut sink, &mut stream, PING_TIMEOUT_CODE).await;
                    return Ended::Disconnected(DisconnectReason::PingTimeout);
                }
                let (seq, ping) = requests.frame(&ClientAction::Ping);
                if let Err(error) = sink.send(ping).await {
                    return Ended::Disconnected(DisconnectReason::Error(error.to_string()));
                }
                pending_ping = Some(seq);
            }
            command = commands.recv() => match command {
                Ok(Command::Send(action)) => {
                    let (_, message) = requests.frame(&action);
                    if let Err(error) = sink.send(message).await {
                        return Ended::Disconnected(DisconnectReason::Error(error.to_string()));
                    }
                }
                Ok(Command::ReconnectNow) => {
                    close(&mut sink, &mut stream, NORMAL_CLOSE_CODE).await;
                    return Ended::Disconnected(DisconnectReason::ReconnectRequested);
                }
                Ok(Command::Close) | Err(_) => {
                    close(&mut sink, &mut stream, NORMAL_CLOSE_CODE).await;
                    return Ended::Closed;
                }
            },
        }
    }
}

/// Sends a close frame and waits briefly for the server's reply, so the server sees the code
/// (dropping a socket with unread data resets it, and the close frame is lost). The webapp
/// doesn't wait for the close handshake either, so the wait is short.
async fn close(sink: &mut SplitSink<Socket, Message>, stream: &mut SplitStream<Socket>, code: u16) {
    let frame = CloseFrame {
        code: CloseCode::from(code),
        reason: "".into(),
    };
    let _ = timeout(Duration::from_secs(1), async {
        if sink.send(Message::Close(Some(frame))).await.is_ok() {
            while let Some(Ok(message)) = stream.next().await {
                if message.is_close() {
                    break;
                }
            }
        }
    })
    .await;
}

#[cfg(test)]
#[path = "connection_tests.rs"]
mod tests;
