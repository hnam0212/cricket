use std::io::ErrorKind;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use cricket_core::audio::{AppId, BackendError, BackendResult, PlaybackState};
use cricket_core::source::{TabBridge, TabId, TabInfo};
use tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tungstenite::http::StatusCode;
use tungstenite::protocol::WebSocketConfig;
use tungstenite::{Message, WebSocket};

use crate::auth;
use crate::protocol::{ClientMessage, CommandKind, ServerMessage, PROTOCOL_VERSION};
use crate::{BIND_HOST, EXTENSION_ID};

/// How long a new connection gets to send a valid `hello`.
const HELLO_TIMEOUT: Duration = Duration::from_secs(5);
/// How long a read waits before the connection thread looks for outgoing
/// messages. Short, so volume steps of a fade go out promptly.
const READ_SLICE: Duration = Duration::from_millis(10);
/// The extension reports the source tab a few times per second. Older
/// than this, the report no longer describes the tab.
const SOURCE_STATE_MAX_AGE: Duration = Duration::from_secs(3);
const DEFAULT_BROWSER: &str = "chrome.exe";
/// Connections handled at once. There is normally exactly one: the
/// extension. A cap keeps a local process from exhausting threads.
const MAX_CONNECTIONS: usize = 8;
/// Largest message or frame accepted. A tab list of thousands of tabs fits.
const MAX_MESSAGE_BYTES: usize = 1 << 20;
/// Largest `hello`: it is read before the peer has proven anything.
const MAX_HELLO_BYTES: usize = 4 << 10;
/// Longest browser name or session id taken from a `hello`.
const MAX_FIELD_CHARS: usize = 64;

/// Finds the browser executable that owns the client end of a connection.
/// Given the client's address as the server sees it and the server's port.
/// Platform specific, so the app supplies it.
pub type PeerResolver = Arc<dyn Fn(SocketAddr, u16) -> Option<AppId> + Send + Sync>;

/// Where the browser executable Cricket uses came from, strongest first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowserSource {
    /// The user picked it.
    Chosen,
    /// Found from the extension's connection to Cricket.
    Detected,
    /// Guessed by the extension from its user agent.
    Reported,
    /// Nothing known; the default.
    Assumed,
}

impl BrowserSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Chosen => "chosen",
            Self::Detected => "detected",
            Self::Reported => "reported",
            Self::Assumed => "assumed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserInfo {
    pub app: AppId,
    pub source: BrowserSource,
}

/// The running bridge. Cheap to clone; all clones share one server.
#[derive(Clone)]
pub struct Bridge {
    shared: Arc<Shared>,
    port: u16,
}

struct Shared {
    /// Replaceable at run time ("generate a new token").
    token: Mutex<String>,
    connections: AtomicUsize,
    port: u16,
    resolver: Option<PeerResolver>,
    /// Set by the user when detection picks the wrong browser.
    browser_override: Mutex<Option<AppId>>,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    /// The paired extension. A newer connection replaces an older one.
    client: Option<Client>,
    /// Remembered so it can be sent again after a reconnect.
    selected: Option<Selection>,
    next_client_id: u64,
}

/// The tab chosen as the music source, with the browser session its id was
/// picked in.
struct Selection {
    tab: TabId,
    session: Option<String>,
}

impl State {
    /// The tab to tell the extension about: the selected one, unless it was
    /// picked in another browser session than the connected one. A tab id
    /// saved before a browser restart can be any tab now.
    fn live_selection(&self) -> Option<TabId> {
        let selection = self.selected.as_ref()?;
        let client = self.client.as_ref()?;
        (selection.session.is_none() || selection.session == client.session)
            .then_some(selection.tab)
    }

    fn selection_stale(&self) -> bool {
        match (&self.selected, &self.client) {
            (Some(selection), Some(client)) => {
                selection.session.is_some() && selection.session != client.session
            }
            _ => false,
        }
    }
}

struct Client {
    id: u64,
    outgoing: Sender<ServerMessage>,
    /// Messages queued for the socket and not yet written to it.
    pending: Arc<AtomicUsize>,
    browser: BrowserInfo,
    /// Identifies the browser run the extension is in.
    session: Option<String>,
    tabs: Vec<TabInfo>,
    source: Option<SourceState>,
}

struct SourceState {
    tab: TabId,
    playing: bool,
    volume: Option<f32>,
    received: Instant,
}

impl Bridge {
    /// Starts listening on 127.0.0.1. Port 0 picks a free port.
    pub fn start(port: u16, token: String) -> std::io::Result<Self> {
        Self::start_with(port, token, None)
    }

    /// Like [`Bridge::start`], with a way to find which browser a
    /// connection comes from.
    pub fn start_with(
        port: u16,
        token: String,
        resolver: Option<PeerResolver>,
    ) -> std::io::Result<Self> {
        let listener = TcpListener::bind((BIND_HOST, port))?;
        let port = listener.local_addr()?.port();
        let shared = Arc::new(Shared {
            token: Mutex::new(token),
            connections: AtomicUsize::new(0),
            port,
            resolver,
            browser_override: Mutex::new(None),
            state: Mutex::new(State::default()),
        });

        let accept_shared = Arc::clone(&shared);
        thread::Builder::new()
            .name("cricket-bridge".to_string())
            .spawn(move || {
                for stream in listener.incoming().flatten() {
                    let shared = Arc::clone(&accept_shared);
                    // One thread per connection, up to a cap.
                    let Some(slot) = ConnectionSlot::take(&shared) else {
                        continue; // dropping the stream closes it
                    };
                    let _ = thread::Builder::new()
                        .name("cricket-bridge-client".to_string())
                        .spawn(move || {
                            serve(stream, &slot.0);
                            drop(slot);
                        });
                }
            })?;

        Ok(Self { shared, port })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// The browser session of the connected extension, if any. Tab ids
    /// picked now are only meaningful within it.
    pub fn current_session(&self) -> Option<String> {
        self.state()
            .client
            .as_ref()
            .and_then(|client| client.session.clone())
    }

    /// Replaces the pairing token. Whoever is connected paired with the old
    /// one, so the connection is closed; the extension has to be given the
    /// new token.
    pub fn set_token(&self, token: String) {
        *self.shared.token.lock().expect("token lock") = token;
        // Dropping the client ends its connection thread.
        self.state().client = None;
    }

    /// Use this browser executable instead of the detected one. `None`
    /// goes back to detection.
    pub fn set_browser_override(&self, browser: Option<AppId>) {
        *self
            .shared
            .browser_override
            .lock()
            .expect("browser override lock") = browser;
    }

    /// The browser whose sound is ignored while a tab is the source, and
    /// how that was decided.
    pub fn browser_info(&self) -> BrowserInfo {
        let chosen = self
            .shared
            .browser_override
            .lock()
            .expect("browser override lock")
            .clone();
        if let Some(app) = chosen {
            return BrowserInfo {
                app,
                source: BrowserSource::Chosen,
            };
        }
        self.state()
            .client
            .as_ref()
            .map(|client| client.browser.clone())
            .unwrap_or_else(|| BrowserInfo {
                app: AppId::new(DEFAULT_BROWSER),
                source: BrowserSource::Assumed,
            })
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.shared.state.lock().expect("bridge state lock")
    }

    fn send(&self, message: ServerMessage) -> BackendResult<()> {
        let state = self.state();
        let client = state.client.as_ref().ok_or_else(not_connected)?;
        queue(client, message)
    }

    /// Waits until everything queued has been written to the socket, or
    /// `timeout` has passed. Used before exiting, so a final "restore the
    /// volume" is not lost in the queue.
    pub fn flush(&self, timeout: Duration) {
        let pending = match self.state().client.as_ref() {
            Some(client) => Arc::clone(&client.pending),
            None => return,
        };
        let deadline = Instant::now() + timeout;
        while pending.load(Ordering::SeqCst) > 0 && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
    }

    fn command(&self, cmd: CommandKind, tab: TabId, volume: Option<f32>) -> BackendResult<()> {
        if self.state().selection_stale() {
            return Err(stale_selection());
        }
        self.send(ServerMessage::Command {
            cmd,
            tab_id: tab,
            volume,
            fade_ms: None,
        })
    }

    fn source_state<T>(
        &self,
        tab: TabId,
        read: impl FnOnce(&SourceState) -> Option<T>,
    ) -> BackendResult<T> {
        let state = self.state();
        let client = state.client.as_ref().ok_or_else(not_connected)?;
        if state.selection_stale() {
            return Err(stale_selection());
        }
        client
            .source
            .as_ref()
            .filter(|source| source.tab == tab)
            .filter(|source| source.received.elapsed() <= SOURCE_STATE_MAX_AGE)
            .and_then(read)
            .ok_or_else(|| BackendError::Unavailable(format!("no report from tab {tab}")))
    }
}

/// Puts a message on a client's outgoing queue, counting it for `flush`.
fn queue(client: &Client, message: ServerMessage) -> BackendResult<()> {
    client.pending.fetch_add(1, Ordering::SeqCst);
    client.outgoing.send(message).map_err(|_| {
        client.pending.fetch_sub(1, Ordering::SeqCst);
        not_connected()
    })
}

fn stale_selection() -> BackendError {
    BackendError::Unavailable(
        "the tab was picked before the browser was restarted; pick it again".to_string(),
    )
}

/// One of the `MAX_CONNECTIONS` places; given back on drop.
struct ConnectionSlot(Arc<Shared>);

impl ConnectionSlot {
    fn take(shared: &Arc<Shared>) -> Option<Self> {
        if shared.connections.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
            shared.connections.fetch_sub(1, Ordering::SeqCst);
            return None;
        }
        Some(Self(Arc::clone(shared)))
    }
}

impl Drop for ConnectionSlot {
    fn drop(&mut self) {
        self.0.connections.fetch_sub(1, Ordering::SeqCst);
    }
}

fn not_connected() -> BackendError {
    BackendError::Unavailable("browser extension not connected".to_string())
}

impl TabBridge for Bridge {
    fn connected(&mut self) -> bool {
        self.state().client.is_some()
    }

    fn browser(&mut self) -> AppId {
        self.browser_info().app
    }

    fn tabs(&mut self) -> Vec<TabInfo> {
        self.state()
            .client
            .as_ref()
            .map(|client| client.tabs.clone())
            .unwrap_or_default()
    }

    fn select(&mut self, tab: Option<TabId>, session: Option<&str>) {
        let mut state = self.state();
        state.selected = tab.map(|tab| Selection {
            tab,
            session: session.map(str::to_string),
        });
        // If nobody is connected, the selection is sent on the next hello.
        let live = state.live_selection();
        if let Some(client) = state.client.as_ref() {
            let _ = queue(client, ServerMessage::SelectSource { tab_id: live });
        }
    }

    fn session(&mut self) -> Option<String> {
        self.current_session()
    }

    fn selection_stale(&mut self) -> bool {
        self.state().selection_stale()
    }

    fn playback_state(&mut self, tab: TabId) -> BackendResult<PlaybackState> {
        self.source_state(tab, |source| {
            Some(if source.playing {
                PlaybackState::Playing
            } else {
                PlaybackState::Paused
            })
        })
    }

    fn volume(&mut self, tab: TabId) -> BackendResult<f32> {
        self.source_state(tab, |source| source.volume)
    }

    fn set_volume(&mut self, tab: TabId, volume: f32) -> BackendResult<()> {
        self.command(CommandKind::SetVolume, tab, Some(volume.clamp(0.0, 1.0)))
    }

    fn pause(&mut self, tab: TabId) -> BackendResult<()> {
        self.command(CommandKind::Pause, tab, None)
    }

    fn play(&mut self, tab: TabId) -> BackendResult<()> {
        self.command(CommandKind::Resume, tab, None)
    }

    fn flush(&mut self, timeout: Duration) {
        Bridge::flush(self, timeout);
    }
}

/// Handles one connection from accept to close.
fn serve(stream: TcpStream, shared: &Shared) {
    // The read timeout bounds the HTTP upgrade and the wait for `hello`.
    if stream.set_read_timeout(Some(HELLO_TIMEOUT)).is_err() {
        return;
    }
    let peer = stream.peer_addr().ok();
    let config = WebSocketConfig::default()
        .max_message_size(Some(MAX_MESSAGE_BYTES))
        .max_frame_size(Some(MAX_MESSAGE_BYTES));
    let Ok(mut socket) = tungstenite::accept_hdr_with_config(stream, check_origin, Some(config))
    else {
        return;
    };
    let Some(paired) = pair(&mut socket, shared) else {
        return;
    };
    // Resolve now, while the connection is certainly in the system's table.
    let detected = peer
        .zip(shared.resolver.as_ref())
        .and_then(|(peer, resolve)| resolve(peer, shared.port));
    let browser = match (detected, paired.browser) {
        (Some(app), _) => BrowserInfo {
            app,
            source: BrowserSource::Detected,
        },
        (None, Some(app)) => BrowserInfo {
            app,
            source: BrowserSource::Reported,
        },
        (None, None) => BrowserInfo {
            app: AppId::new(DEFAULT_BROWSER),
            source: BrowserSource::Assumed,
        },
    };
    if socket.get_ref().set_read_timeout(Some(READ_SLICE)).is_err() {
        return;
    }

    let (outgoing, incoming_commands) = mpsc::channel();
    let pending = Arc::new(AtomicUsize::new(0));
    let id = {
        let mut state = shared.state.lock().expect("bridge state lock");
        let id = state.next_client_id;
        state.next_client_id += 1;
        // Replacing the old client drops its sender, which ends its thread.
        state.client = Some(Client {
            id,
            outgoing,
            pending: Arc::clone(&pending),
            browser,
            session: paired.session,
            tabs: Vec::new(),
            source: None,
        });
        // Tell the extension the current choice right away; it does not
        // remember it across restarts of its service worker.
        let live = state.live_selection();
        if let Some(client) = state.client.as_ref() {
            let _ = queue(client, ServerMessage::SelectSource { tab_id: live });
        }
        id
    };

    pump(&mut socket, shared, id, &incoming_commands, &pending);

    let mut state = shared.state.lock().expect("bridge state lock");
    if state.client.as_ref().is_some_and(|client| client.id == id) {
        state.client = None;
    }
}

/// Web pages can open WebSockets to localhost, and so can other browser
/// extensions. Browsers always attach the opener's origin, so anything but
/// Cricket's own extension is turned away before a single message is read.
/// (A connection without an origin is not a browser; it still has to prove
/// it knows the token.)
// The signature, including the large error type, is dictated by tungstenite.
#[allow(clippy::result_large_err)]
fn check_origin(request: &Request, response: Response) -> Result<Response, ErrorResponse> {
    let allowed = match request.headers().get("origin") {
        None => true,
        Some(origin) => origin
            .to_str()
            .is_ok_and(|origin| origin == format!("chrome-extension://{EXTENSION_ID}")),
    };
    if allowed {
        Ok(response)
    } else {
        let mut refused = ErrorResponse::new(Some("origin not allowed".to_string()));
        *refused.status_mut() = StatusCode::FORBIDDEN;
        Err(refused)
    }
}

/// What a successful `hello` tells about the extension.
struct Paired {
    /// The browser executable the extension claims to run in.
    browser: Option<AppId>,
    session: Option<String>,
}

/// Runs the challenge-response. `None` if the peer did not pass.
fn pair(socket: &mut WebSocket<TcpStream>, shared: &Shared) -> Option<Paired> {
    let server_nonce = auth::random_nonce().ok()?;
    send(
        socket,
        &ServerMessage::Challenge {
            version: PROTOCOL_VERSION,
            nonce: server_nonce.clone(),
        },
    )
    .ok()?;

    let text = loop {
        match socket.read().ok()? {
            Message::Text(text) => break text,
            Message::Ping(_) | Message::Pong(_) => continue,
            _ => return None,
        }
    };

    let token = shared.token.lock().expect("token lock").clone();
    match check_hello(&text, &server_nonce, &token) {
        Ok((paired, proof)) => {
            send(
                socket,
                &ServerMessage::HelloAck {
                    ok: true,
                    error: None,
                    proof: Some(proof),
                },
            )
            .ok()?;
            Some(paired)
        }
        Err(error) => {
            let _ = send(
                socket,
                &ServerMessage::HelloAck {
                    ok: false,
                    error: Some(error),
                    proof: None,
                },
            );
            let _ = socket.close(None);
            let _ = socket.flush();
            None
        }
    }
}

/// Validates a `hello`. On success also returns the app's proof for the
/// extension to check.
fn check_hello(text: &str, server_nonce: &str, token: &str) -> Result<(Paired, String), String> {
    if text.len() > MAX_HELLO_BYTES {
        return Err("hello is too large".to_string());
    }
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|_| "expected hello".to_string())?;
    if value.get("type").and_then(serde_json::Value::as_str) != Some("hello") {
        return Err("expected hello".to_string());
    }
    // Looked at before the rest is parsed, so an older extension gets an
    // answer it can show instead of a parse error.
    let version = value.get("version").and_then(serde_json::Value::as_u64);
    if version != Some(u64::from(PROTOCOL_VERSION)) {
        let theirs = version.map_or_else(|| "?".to_string(), |v| v.to_string());
        return Err(format!(
            "protocol version {theirs} is not supported (need {PROTOCOL_VERSION}); \
             update the extension and Cricket together"
        ));
    }
    let Ok(ClientMessage::Hello {
        client_nonce,
        proof,
        session,
        browser,
        ..
    }) = serde_json::from_value(value)
    else {
        return Err("malformed hello".to_string());
    };
    if client_nonce.is_empty() || client_nonce.len() > MAX_FIELD_CHARS {
        return Err("malformed hello".to_string());
    }
    if !auth::verify_client_proof(token, server_nonce, &client_nonce, &proof) {
        return Err("wrong pairing token".to_string());
    }
    let paired = Paired {
        browser: short(browser).map(|name| AppId::new(&name)),
        session: short(session),
    };
    Ok((
        paired,
        auth::server_proof(token, server_nonce, &client_nonce),
    ))
}

/// A text field from the peer, if it is a sensible size.
fn short(field: Option<String>) -> Option<String> {
    field.filter(|text| !text.is_empty() && text.len() <= MAX_FIELD_CHARS)
}

/// Moves messages both ways until the connection ends or is replaced.
fn pump(
    socket: &mut WebSocket<TcpStream>,
    shared: &Shared,
    id: u64,
    outgoing: &Receiver<ServerMessage>,
    pending: &AtomicUsize,
) {
    loop {
        match socket.read() {
            Ok(Message::Text(text)) => {
                // Unknown or malformed messages are skipped, so a newer
                // extension can add message types without breaking this.
                if let Ok(message) = serde_json::from_str::<ClientMessage>(&text) {
                    apply(shared, id, message);
                }
            }
            Ok(Message::Close(_)) => return,
            Ok(_) => {}
            Err(tungstenite::Error::Io(error))
                if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(_) => return,
        }

        loop {
            match outgoing.try_recv() {
                Ok(message) => {
                    let sent = send(socket, &message);
                    pending.fetch_sub(1, Ordering::SeqCst);
                    if sent.is_err() {
                        return;
                    }
                }
                Err(TryRecvError::Empty) => break,
                // A newer connection took over.
                Err(TryRecvError::Disconnected) => {
                    let _ = socket.close(None);
                    let _ = socket.flush();
                    return;
                }
            }
        }
    }
}

fn apply(shared: &Shared, id: u64, message: ClientMessage) {
    let mut state = shared.state.lock().expect("bridge state lock");
    let Some(client) = state.client.as_mut().filter(|client| client.id == id) else {
        return;
    };
    match message {
        ClientMessage::Tabs { tabs } => client.tabs = tabs,
        ClientMessage::SourceState {
            tab_id,
            playing,
            volume,
        } => {
            client.source = Some(SourceState {
                tab: tab_id,
                playing,
                volume: volume.filter(|v| v.is_finite()).map(|v| v.clamp(0.0, 1.0)),
                received: Instant::now(),
            });
        }
        // A second hello changes nothing; ping only keeps the link alive.
        ClientMessage::Hello { .. } | ClientMessage::Ping => {}
    }
}

fn send(socket: &mut WebSocket<TcpStream>, message: &ServerMessage) -> tungstenite::Result<()> {
    let json = serde_json::to_string(message).expect("server message serializes");
    socket.send(Message::text(json))
}

#[cfg(test)]
mod tests {
    use tungstenite::client::IntoClientRequest;
    use tungstenite::stream::MaybeTlsStream;

    use super::*;

    const TOKEN: &str = "0123456789abcdef0123456789abcdef";

    type Client = WebSocket<MaybeTlsStream<TcpStream>>;

    fn start() -> Bridge {
        Bridge::start(0, TOKEN.to_string()).unwrap()
    }

    fn connect(bridge: &Bridge) -> Client {
        let (socket, _) =
            tungstenite::connect(format!("ws://127.0.0.1:{}", bridge.port())).unwrap();
        if let MaybeTlsStream::Plain(stream) = socket.get_ref() {
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
        }
        socket
    }

    fn say(client: &mut Client, json: &str) {
        client.send(Message::text(json)).unwrap();
    }

    fn hear(client: &mut Client) -> serde_json::Value {
        loop {
            if let Message::Text(text) = client.read().unwrap() {
                return serde_json::from_str(&text).unwrap();
            }
        }
    }

    const CLIENT_NONCE: &str = "0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f";

    /// Reads the challenge and answers with a hello that proves `token`.
    /// `extra` is spliced into the hello as more JSON fields. Returns the
    /// app's answer, after checking its proof when it accepted.
    fn handshake(client: &mut Client, token: &str, extra: &str) -> serde_json::Value {
        let challenge = hear(client);
        assert_eq!(challenge["type"], "challenge");
        assert_eq!(challenge["version"], PROTOCOL_VERSION);
        let server_nonce = challenge["nonce"].as_str().unwrap().to_string();
        let proof = auth::client_proof(token, &server_nonce, CLIENT_NONCE);
        say(
            client,
            &format!(
                r#"{{"type":"hello","version":2,"clientNonce":"{CLIENT_NONCE}","proof":"{proof}"{extra}}}"#
            ),
        );
        let ack = hear(client);
        if ack["ok"] == true {
            assert_eq!(
                ack["proof"],
                auth::server_proof(token, &server_nonce, CLIENT_NONCE)
            );
        }
        ack
    }

    /// Connects and pairs, consuming the ack and the initial selection.
    fn paired(bridge: &Bridge) -> Client {
        paired_with(bridge, "")
    }

    fn paired_with(bridge: &Bridge, extra: &str) -> Client {
        let mut client = connect(bridge);
        assert_eq!(handshake(&mut client, TOKEN, extra)["ok"], true);
        assert_eq!(hear(&mut client)["type"], "select_source");
        client
    }

    fn wait_until(what: &str, mut condition: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !condition() {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn pairs_with_the_right_token() {
        let mut bridge = start();
        assert!(!bridge.connected());

        let _client = paired(&bridge);

        wait_until("connected", || bridge.connected());
    }

    #[test]
    fn refuses_a_wrong_token() {
        let mut bridge = start();
        let mut client = connect(&bridge);

        let ack = handshake(&mut client, "nope", "");

        assert_eq!(ack["type"], "hello_ack");
        assert!(ack.get("proof").is_none());
        assert_eq!(ack["ok"], false);
        assert_eq!(ack["error"], "wrong pairing token");
        assert!(!bridge.connected());
    }

    #[test]
    fn refuses_anything_before_hello() {
        let mut bridge = start();
        let mut client = connect(&bridge);
        assert_eq!(hear(&mut client)["type"], "challenge");

        say(&mut client, r#"{"type":"tabs","tabs":[]}"#);

        assert_eq!(hear(&mut client)["ok"], false);
        assert!(!bridge.connected());
    }

    #[test]
    fn the_token_itself_is_never_sent_or_accepted() {
        let mut bridge = start();
        let mut client = connect(&bridge);
        assert_eq!(hear(&mut client)["type"], "challenge");

        // A version 1 style hello that carries the right token in clear.
        say(
            &mut client,
            &format!(r#"{{"type":"hello","version":1,"token":"{TOKEN}"}}"#),
        );
        let ack = hear(&mut client);

        assert_eq!(ack["ok"], false);
        assert!(ack["error"].as_str().unwrap().contains("update"));
        assert!(!bridge.connected());
    }

    #[test]
    fn a_proof_from_another_connection_does_not_replay() {
        let mut bridge = start();
        let mut first = connect(&bridge);
        let first_nonce = hear(&mut first)["nonce"].as_str().unwrap().to_string();
        let stolen = auth::client_proof(TOKEN, &first_nonce, CLIENT_NONCE);

        let mut second = connect(&bridge);
        assert_eq!(hear(&mut second)["type"], "challenge");
        say(
            &mut second,
            &format!(
                r#"{{"type":"hello","version":2,"clientNonce":"{CLIENT_NONCE}","proof":"{stolen}"}}"#
            ),
        );

        assert_eq!(hear(&mut second)["ok"], false);
        assert!(!bridge.connected());
    }

    #[test]
    fn an_oversized_hello_is_refused() {
        let mut bridge = start();
        let mut client = connect(&bridge);
        assert_eq!(hear(&mut client)["type"], "challenge");

        let padding = "x".repeat(MAX_HELLO_BYTES);
        say(
            &mut client,
            &format!(
                r#"{{"type":"hello","version":2,"clientNonce":"n","proof":"p","browser":"{padding}"}}"#
            ),
        );

        assert_eq!(hear(&mut client)["ok"], false);
        assert!(!bridge.connected());
    }

    #[test]
    fn a_message_over_the_size_limit_closes_the_connection() {
        let mut bridge = start();
        let mut client = paired(&bridge);
        wait_until("connected", || bridge.connected());

        let padding = "x".repeat(MAX_MESSAGE_BYTES + 1);
        let _ = client.send(Message::text(format!(
            r#"{{"type":"tabs","tabs":[{{"id":1,"title":"{padding}"}}]}}"#
        )));

        wait_until("disconnected", || !bridge.connected());
    }

    #[test]
    fn connections_beyond_the_cap_are_dropped() {
        let bridge = start();
        // Idle connections that never say hello occupy their slots.
        let idle: Vec<_> = (0..MAX_CONNECTIONS).map(|_| connect(&bridge)).collect();
        wait_until("slots taken", || {
            bridge.shared.connections.load(Ordering::SeqCst) == MAX_CONNECTIONS
        });

        // The next one never gets a challenge: it is closed straight away.
        let refused = tungstenite::connect(format!("ws://127.0.0.1:{}", bridge.port()));
        assert!(refused.is_err());
        drop(idle);
    }

    #[test]
    fn refuses_an_unsupported_protocol_version() {
        let bridge = start();
        let mut client = connect(&bridge);

        assert_eq!(hear(&mut client)["type"], "challenge");
        say(
            &mut client,
            r#"{"type":"hello","version":99,"clientNonce":"n","proof":"p"}"#,
        );

        assert_eq!(hear(&mut client)["ok"], false);
    }

    #[test]
    fn refuses_connections_from_web_pages() {
        let bridge = start();
        let mut request = format!("ws://127.0.0.1:{}", bridge.port())
            .into_client_request()
            .unwrap();
        request
            .headers_mut()
            .insert("origin", "https://evil.example".parse().unwrap());

        assert!(tungstenite::connect(request).is_err());
    }

    #[test]
    fn accepts_connections_from_an_extension() {
        let bridge = start();
        let mut request = format!("ws://127.0.0.1:{}", bridge.port())
            .into_client_request()
            .unwrap();
        request.headers_mut().insert(
            "origin",
            format!("chrome-extension://{EXTENSION_ID}")
                .parse()
                .unwrap(),
        );

        let (mut socket, _) = tungstenite::connect(request).unwrap();
        assert_eq!(hear(&mut socket)["type"], "challenge");
    }

    #[test]
    fn refuses_other_extensions() {
        let bridge = start();
        for origin in [
            "chrome-extension://abcdefghijklmnopabcdefghijklmnop".to_string(),
            format!("chrome-extension://{EXTENSION_ID}.evil"),
            format!("chrome-extension://{EXTENSION_ID}/"),
        ] {
            let mut request = format!("ws://127.0.0.1:{}", bridge.port())
                .into_client_request()
                .unwrap();
            request
                .headers_mut()
                .insert("origin", origin.parse().unwrap());
            assert!(tungstenite::connect(request).is_err(), "{origin}");
        }
    }

    #[test]
    fn reports_tabs_and_the_browser() {
        let mut bridge = start();
        let mut client = paired_with(&bridge, r#","browser":"msedge.exe""#);

        say(
            &mut client,
            r#"{"type":"tabs","tabs":[{"id":1,"title":"Music","audible":true},{"id":2,"title":"Docs","audible":false}]}"#,
        );

        wait_until("tabs", || bridge.tabs().len() == 2);
        assert_eq!(bridge.tabs()[0].title, "Music");
        assert!(bridge.tabs()[0].audible);
        assert_eq!(bridge.browser(), AppId::new("msedge.exe"));
    }

    fn hello_with_browser(bridge: &Bridge, browser: &str) -> Client {
        let mut client = connect(bridge);
        let extra = format!(r#","browser":"{browser}""#);
        assert_eq!(handshake(&mut client, TOKEN, &extra)["ok"], true);
        client
    }

    fn start_resolving(found: Option<&'static str>) -> Bridge {
        let resolver: PeerResolver = Arc::new(move |_peer, _port| found.map(AppId::new));
        Bridge::start_with(0, TOKEN.to_string(), Some(resolver)).unwrap()
    }

    #[test]
    fn the_detected_browser_beats_the_extensions_guess() {
        let bridge = start_resolving(Some("brave.exe"));
        let _client = hello_with_browser(&bridge, "chrome.exe");
        wait_until("connected", || bridge.state().client.is_some());

        let info = bridge.browser_info();
        assert_eq!(info.app, AppId::new("brave.exe"));
        assert_eq!(info.source, BrowserSource::Detected);
    }

    #[test]
    fn without_a_detection_the_extensions_guess_is_used() {
        let bridge = start_resolving(None);
        let _client = hello_with_browser(&bridge, "msedge.exe");
        wait_until("connected", || bridge.state().client.is_some());

        let info = bridge.browser_info();
        assert_eq!(info.app, AppId::new("msedge.exe"));
        assert_eq!(info.source, BrowserSource::Reported);
    }

    #[test]
    fn the_browser_chosen_by_the_user_beats_everything() {
        let bridge = start_resolving(Some("brave.exe"));
        let _client = hello_with_browser(&bridge, "chrome.exe");
        wait_until("connected", || bridge.state().client.is_some());

        bridge.set_browser_override(Some(AppId::new("vivaldi.exe")));
        assert_eq!(bridge.browser_info().app, AppId::new("vivaldi.exe"));
        assert_eq!(bridge.browser_info().source, BrowserSource::Chosen);

        bridge.set_browser_override(None);
        assert_eq!(bridge.browser_info().source, BrowserSource::Detected);
    }

    #[test]
    fn with_no_connection_the_browser_is_assumed() {
        let bridge = start();
        assert_eq!(bridge.browser_info().source, BrowserSource::Assumed);
        assert_eq!(bridge.browser_info().app, AppId::new(DEFAULT_BROWSER));
    }

    #[test]
    fn reports_the_source_tab_state() {
        let mut bridge = start();
        let mut client = paired(&bridge);
        assert!(bridge.playback_state(TabId(1)).is_err());

        say(
            &mut client,
            r#"{"type":"source_state","tabId":1,"playing":true,"volume":0.5}"#,
        );
        wait_until("source state", || bridge.playback_state(TabId(1)).is_ok());

        assert_eq!(bridge.playback_state(TabId(1)), Ok(PlaybackState::Playing));
        assert_eq!(bridge.volume(TabId(1)), Ok(0.5));
        // A report about one tab says nothing about another.
        assert!(bridge.playback_state(TabId(2)).is_err());

        say(
            &mut client,
            r#"{"type":"source_state","tabId":1,"playing":false}"#,
        );
        wait_until("paused", || {
            bridge.playback_state(TabId(1)) == Ok(PlaybackState::Paused)
        });
        assert!(bridge.volume(TabId(1)).is_err());
    }

    #[test]
    fn sends_commands_to_the_extension() {
        let mut bridge = start();
        let mut client = paired(&bridge);
        wait_until("connected", || bridge.connected());

        bridge.set_volume(TabId(7), 0.25).unwrap();
        bridge.pause(TabId(7)).unwrap();
        bridge.play(TabId(7)).unwrap();

        assert_eq!(
            hear(&mut client),
            serde_json::json!({"type":"command","cmd":"set_volume","tabId":7,"volume":0.25})
        );
        assert_eq!(
            hear(&mut client),
            serde_json::json!({"type":"command","cmd":"pause","tabId":7})
        );
        assert_eq!(
            hear(&mut client),
            serde_json::json!({"type":"command","cmd":"resume","tabId":7})
        );
    }

    #[test]
    fn flush_waits_until_queued_commands_are_written() {
        let mut bridge = start();
        let mut client = paired(&bridge);
        wait_until("connected", || bridge.connected());

        bridge.set_volume(TabId(7), 0.9).unwrap();
        bridge.flush(Duration::from_secs(2));

        let pending = {
            let state = bridge.state();
            state
                .client
                .as_ref()
                .unwrap()
                .pending
                .load(Ordering::SeqCst)
        };
        assert_eq!(pending, 0);
        assert_eq!(
            hear(&mut client),
            serde_json::json!({"type":"command","cmd":"set_volume","tabId":7,"volume":0.9})
        );
    }

    #[test]
    fn flush_returns_at_once_when_nothing_is_connected() {
        let bridge = start();
        let started = Instant::now();
        bridge.flush(Duration::from_secs(5));
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn commands_fail_while_nothing_is_connected() {
        let mut bridge = start();

        assert!(matches!(
            bridge.pause(TabId(1)),
            Err(BackendError::Unavailable(_))
        ));
        assert!(bridge.tabs().is_empty());
    }

    #[test]
    fn a_selection_made_before_connecting_is_sent_on_hello() {
        let mut bridge = start();
        bridge.select(Some(TabId(9)), None);

        let mut client = connect(&bridge);
        assert_eq!(handshake(&mut client, TOKEN, "")["ok"], true);
        assert_eq!(
            hear(&mut client),
            serde_json::json!({"type":"select_source","tabId":9})
        );
    }

    #[test]
    fn a_tab_picked_in_the_same_browser_session_is_restored() {
        let mut bridge = start();
        bridge.select(Some(TabId(9)), Some("run-1"));

        let mut client = connect(&bridge);
        assert_eq!(
            handshake(&mut client, TOKEN, r#","session":"run-1""#)["ok"],
            true
        );
        assert_eq!(
            hear(&mut client),
            serde_json::json!({"type":"select_source","tabId":9})
        );
        assert!(!bridge.selection_stale());
        assert_eq!(bridge.session().as_deref(), Some("run-1"));
    }

    #[test]
    fn a_tab_from_an_earlier_browser_session_is_not_selected() {
        let mut bridge = start();
        // Picked before the browser was restarted: tab 9 then is not tab 9 now.
        bridge.select(Some(TabId(9)), Some("run-1"));

        let mut client = connect(&bridge);
        assert_eq!(
            handshake(&mut client, TOKEN, r#","session":"run-2""#)["ok"],
            true
        );

        assert_eq!(
            hear(&mut client),
            serde_json::json!({"type":"select_source","tabId":null})
        );
        wait_until("connected", || bridge.connected());
        assert!(bridge.selection_stale());
        // Nothing is read from or sent to whatever tab 9 is now.
        assert!(bridge.pause(TabId(9)).is_err());
        assert!(bridge.playback_state(TabId(9)).is_err());
    }

    #[test]
    fn a_tab_saved_without_a_session_is_still_selected() {
        let mut bridge = start();
        bridge.select(Some(TabId(9)), None);

        let mut client = connect(&bridge);
        assert_eq!(
            handshake(&mut client, TOKEN, r#","session":"run-2""#)["ok"],
            true
        );
        assert_eq!(hear(&mut client)["tabId"], 9);
        assert!(!bridge.selection_stale());
    }

    #[test]
    fn a_new_token_disconnects_the_extension_and_is_required_afterwards() {
        let mut bridge = start();
        let _client = paired(&bridge);
        wait_until("connected", || bridge.connected());

        bridge.set_token("fedcba9876543210fedcba9876543210".to_string());
        wait_until("disconnected", || !bridge.connected());

        let mut old = connect(&bridge);
        assert_eq!(handshake(&mut old, TOKEN, "")["ok"], false);
        let mut new = connect(&bridge);
        assert_eq!(
            handshake(&mut new, "fedcba9876543210fedcba9876543210", "")["ok"],
            true
        );
    }

    #[test]
    fn a_selection_made_while_connected_is_sent_at_once() {
        let mut bridge = start();
        let mut client = paired(&bridge);
        wait_until("connected", || bridge.connected());

        bridge.select(Some(TabId(3)), None);
        assert_eq!(
            hear(&mut client),
            serde_json::json!({"type":"select_source","tabId":3})
        );

        bridge.select(None, None);
        assert_eq!(
            hear(&mut client),
            serde_json::json!({"type":"select_source","tabId":null})
        );
    }

    #[test]
    fn disconnecting_clears_the_tabs() {
        let mut bridge = start();
        let mut client = paired(&bridge);
        say(&mut client, r#"{"type":"tabs","tabs":[{"id":1}]}"#);
        wait_until("tabs", || bridge.tabs().len() == 1);

        client.close(None).unwrap();
        drop(client);

        wait_until("disconnected", || !bridge.connected());
        assert!(bridge.tabs().is_empty());
    }

    #[test]
    fn a_new_connection_replaces_the_old_one() {
        let mut bridge = start();
        let mut first = paired(&bridge);
        say(&mut first, r#"{"type":"tabs","tabs":[{"id":1}]}"#);
        wait_until("first tabs", || bridge.tabs().len() == 1);

        let mut second = paired(&bridge);
        say(&mut second, r#"{"type":"tabs","tabs":[{"id":1},{"id":2}]}"#);

        wait_until("second tabs", || bridge.tabs().len() == 2);
        assert!(bridge.connected());
    }

    #[test]
    fn malformed_messages_are_ignored() {
        let mut bridge = start();
        let mut client = paired(&bridge);

        say(&mut client, "not json");
        say(&mut client, r#"{"type":"something_new","x":1}"#);
        say(&mut client, r#"{"type":"ping"}"#);
        say(&mut client, r#"{"type":"tabs","tabs":[{"id":4}]}"#);

        wait_until("tabs after junk", || bridge.tabs().len() == 1);
    }
}
