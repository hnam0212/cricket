use std::io::ErrorKind;
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use cricket_core::audio::{AppId, BackendError, BackendResult, PlaybackState};
use cricket_core::source::{TabBridge, TabId, TabInfo};
use tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tungstenite::http::StatusCode;
use tungstenite::{Message, WebSocket};

use crate::protocol::{ClientMessage, CommandKind, ServerMessage, PROTOCOL_VERSION};
use crate::BIND_HOST;

/// How long a new connection gets to send a valid `hello`.
const HELLO_TIMEOUT: Duration = Duration::from_secs(5);
/// How long a read waits before the connection thread looks for outgoing
/// messages. Short, so volume steps of a fade go out promptly.
const READ_SLICE: Duration = Duration::from_millis(10);
/// The extension reports the source tab a few times per second. Older
/// than this, the report no longer describes the tab.
const SOURCE_STATE_MAX_AGE: Duration = Duration::from_secs(3);
const DEFAULT_BROWSER: &str = "chrome.exe";

/// The running bridge. Cheap to clone; all clones share one server.
#[derive(Clone)]
pub struct Bridge {
    shared: Arc<Shared>,
    port: u16,
}

struct Shared {
    token: String,
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
        (!self.selection_stale()).then_some(selection.tab)
    }

    fn selection_stale(&self) -> bool {
        match (&self.selected, &self.client) {
            // A selection saved without a session predates sessions and is
            // trusted as before.
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
    browser: AppId,
    /// Identifies the browser run the extension is in. `None` from an
    /// extension that does not report one.
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
        let listener = TcpListener::bind((BIND_HOST, port))?;
        let port = listener.local_addr()?.port();
        let shared = Arc::new(Shared {
            token,
            state: Mutex::new(State::default()),
        });

        let accept_shared = Arc::clone(&shared);
        thread::Builder::new()
            .name("cricket-bridge".to_string())
            .spawn(move || {
                for stream in listener.incoming().flatten() {
                    let shared = Arc::clone(&accept_shared);
                    // One thread per connection. There is normally exactly
                    // one: the extension.
                    let _ = thread::Builder::new()
                        .name("cricket-bridge-client".to_string())
                        .spawn(move || serve(stream, &shared));
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

    fn state(&self) -> MutexGuard<'_, State> {
        self.shared.state.lock().expect("bridge state lock")
    }

    fn send(&self, message: ServerMessage) -> BackendResult<()> {
        let state = self.state();
        let client = state.client.as_ref().ok_or_else(not_connected)?;
        client.pending.fetch_add(1, Ordering::SeqCst);
        client.outgoing.send(message).map_err(|_| {
            client.pending.fetch_sub(1, Ordering::SeqCst);
            not_connected()
        })
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

fn not_connected() -> BackendError {
    BackendError::Unavailable("browser extension not connected".to_string())
}

fn stale_selection() -> BackendError {
    BackendError::Unavailable(
        "the tab was picked before the browser was restarted; pick it again".to_string(),
    )
}

impl TabBridge for Bridge {
    fn connected(&mut self) -> bool {
        self.state().client.is_some()
    }

    fn browser(&mut self) -> AppId {
        self.state()
            .client
            .as_ref()
            .map(|client| client.browser.clone())
            .unwrap_or_else(|| AppId::new(DEFAULT_BROWSER))
    }

    fn tabs(&mut self) -> Vec<TabInfo> {
        self.state()
            .client
            .as_ref()
            .map(|client| client.tabs.clone())
            .unwrap_or_default()
    }

    fn select(&mut self, tab: Option<TabId>, session: Option<&str>) {
        let live = {
            let mut state = self.state();
            state.selected = tab.map(|tab| Selection {
                tab,
                session: session.map(str::to_string),
            });
            state.live_selection()
        };
        // If nobody is connected, the selection is sent on the next hello.
        let _ = self.send(ServerMessage::SelectSource { tab_id: live });
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
    let Ok(mut socket) = tungstenite::accept_hdr(stream, check_origin) else {
        return;
    };
    let Some(paired) = pair(&mut socket, shared) else {
        return;
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
        let to_client = outgoing.clone();
        // Replacing the old client drops its sender, which ends its thread.
        state.client = Some(Client {
            id,
            outgoing,
            pending: Arc::clone(&pending),
            browser: paired.browser,
            session: paired.session,
            tabs: Vec::new(),
            source: None,
        });
        // Tell the extension the current choice right away; it does not
        // remember it across restarts of its service worker. Decided after
        // the client is in place, because it depends on its session.
        pending.fetch_add(1, Ordering::SeqCst);
        let _ = to_client.send(ServerMessage::SelectSource {
            tab_id: state.live_selection(),
        });
        id
    };

    pump(&mut socket, shared, id, &incoming_commands, &pending);

    let mut state = shared.state.lock().expect("bridge state lock");
    if state.client.as_ref().is_some_and(|client| client.id == id) {
        state.client = None;
    }
}

/// Web pages can open WebSockets to localhost. Browsers always attach the
/// page's origin, so anything that is not an extension is turned away before
/// the token is even looked at.
// The signature, including the large error type, is dictated by tungstenite.
#[allow(clippy::result_large_err)]
fn check_origin(request: &Request, response: Response) -> Result<Response, ErrorResponse> {
    let allowed = match request.headers().get("origin") {
        // Not a browser (a native test client).
        None => true,
        Some(origin) => origin
            .to_str()
            .is_ok_and(|origin| origin.starts_with("chrome-extension://")),
    };
    if allowed {
        Ok(response)
    } else {
        let mut refused = ErrorResponse::new(Some("origin not allowed".to_string()));
        *refused.status_mut() = StatusCode::FORBIDDEN;
        Err(refused)
    }
}

/// Longest session id taken from a `hello`; the extension sends a UUID.
const MAX_SESSION_CHARS: usize = 64;

/// What a valid `hello` told us about the extension.
struct Paired {
    /// The browser's executable.
    browser: AppId,
    session: Option<String>,
}

/// Waits for a valid `hello`.
fn pair(socket: &mut WebSocket<TcpStream>, shared: &Shared) -> Option<Paired> {
    let hello = loop {
        match socket.read().ok()? {
            Message::Text(text) => break serde_json::from_str::<ClientMessage>(&text).ok(),
            Message::Ping(_) | Message::Pong(_) => continue,
            _ => break None,
        }
    };

    let verdict = match hello {
        Some(ClientMessage::Hello {
            version,
            token,
            session,
            browser,
        }) => {
            if version != PROTOCOL_VERSION {
                Err(format!(
                    "protocol version {version} is not supported; update the extension or Cricket"
                ))
            } else if !tokens_match(&token, &shared.token) {
                Err("wrong pairing token".to_string())
            } else {
                Ok(Paired {
                    browser: AppId::new(browser.as_deref().unwrap_or(DEFAULT_BROWSER)),
                    session: session
                        .filter(|id| !id.is_empty() && id.chars().count() <= MAX_SESSION_CHARS),
                })
            }
        }
        _ => Err("expected hello".to_string()),
    };

    match verdict {
        Ok(paired) => {
            send(
                socket,
                &ServerMessage::HelloAck {
                    ok: true,
                    error: None,
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
                },
            );
            let _ = socket.close(None);
            let _ = socket.flush();
            None
        }
    }
}

/// Compares without stopping at the first difference, so response time does
/// not reveal how much of a guessed token was right.
fn tokens_match(given: &str, expected: &str) -> bool {
    if expected.is_empty() || given.len() != expected.len() {
        return false;
    }
    given
        .bytes()
        .zip(expected.bytes())
        .fold(0u8, |difference, (a, b)| difference | (a ^ b))
        == 0
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

    /// Connects and pairs, consuming the ack and the initial selection.
    fn paired(bridge: &Bridge) -> Client {
        let mut client = connect(bridge);
        say(
            &mut client,
            &format!(r#"{{"type":"hello","version":1,"token":"{TOKEN}"}}"#),
        );
        assert_eq!(hear(&mut client)["ok"], true);
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

        say(
            &mut client,
            r#"{"type":"hello","version":1,"token":"nope"}"#,
        );
        let ack = hear(&mut client);

        assert_eq!(ack["type"], "hello_ack");
        assert_eq!(ack["ok"], false);
        assert_eq!(ack["error"], "wrong pairing token");
        assert!(!bridge.connected());
    }

    #[test]
    fn refuses_anything_before_hello() {
        let mut bridge = start();
        let mut client = connect(&bridge);

        say(&mut client, r#"{"type":"tabs","tabs":[]}"#);

        assert_eq!(hear(&mut client)["ok"], false);
        assert!(!bridge.connected());
    }

    #[test]
    fn refuses_an_unsupported_protocol_version() {
        let bridge = start();
        let mut client = connect(&bridge);

        say(
            &mut client,
            &format!(r#"{{"type":"hello","version":99,"token":"{TOKEN}"}}"#),
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
            "chrome-extension://abcdefghijklmnop".parse().unwrap(),
        );

        assert!(tungstenite::connect(request).is_ok());
    }

    #[test]
    fn an_empty_expected_token_never_matches() {
        assert!(!tokens_match("", ""));
        assert!(!tokens_match("abc", ""));
        assert!(tokens_match("abc", "abc"));
        assert!(!tokens_match("abd", "abc"));
        assert!(!tokens_match("abcd", "abc"));
    }

    #[test]
    fn reports_tabs_and_the_browser() {
        let mut bridge = start();
        let mut client = connect(&bridge);
        say(
            &mut client,
            &format!(r#"{{"type":"hello","version":1,"token":"{TOKEN}","browser":"msedge.exe"}}"#),
        );
        hear(&mut client);

        say(
            &mut client,
            r#"{"type":"tabs","tabs":[{"id":1,"title":"Music","url":"https://m","audible":true},{"id":2,"title":"Docs","url":"https://d","audible":false}]}"#,
        );

        wait_until("tabs", || bridge.tabs().len() == 2);
        assert_eq!(bridge.tabs()[0].title, "Music");
        assert!(bridge.tabs()[0].audible);
        assert_eq!(bridge.browser(), AppId::new("msedge.exe"));
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
        say(
            &mut client,
            &format!(r#"{{"type":"hello","version":1,"token":"{TOKEN}"}}"#),
        );

        assert_eq!(hear(&mut client)["ok"], true);
        assert_eq!(
            hear(&mut client),
            serde_json::json!({"type":"select_source","tabId":9})
        );
    }

    /// Connects and pairs as an extension in the given browser session,
    /// returning the initial selection it is told about.
    fn paired_in_session(bridge: &Bridge, session: &str) -> (Client, serde_json::Value) {
        let mut client = connect(bridge);
        say(
            &mut client,
            &format!(r#"{{"type":"hello","version":1,"token":"{TOKEN}","session":"{session}"}}"#),
        );
        assert_eq!(hear(&mut client)["ok"], true);
        let selection = hear(&mut client);
        (client, selection)
    }

    #[test]
    fn a_tab_picked_in_the_same_browser_session_is_restored() {
        let mut bridge = start();
        bridge.select(Some(TabId(9)), Some("run-1"));

        let (_client, selection) = paired_in_session(&bridge, "run-1");

        assert_eq!(
            selection,
            serde_json::json!({"type":"select_source","tabId":9})
        );
        assert!(!bridge.selection_stale());
        assert_eq!(bridge.current_session().as_deref(), Some("run-1"));
    }

    #[test]
    fn a_tab_from_an_earlier_browser_session_is_not_selected() {
        let mut bridge = start();
        // Picked before the browser was restarted: tab 9 then is not tab 9 now.
        bridge.select(Some(TabId(9)), Some("run-1"));

        let (mut client, selection) = paired_in_session(&bridge, "run-2");

        assert_eq!(
            selection,
            serde_json::json!({"type":"select_source","tabId":null})
        );
        wait_until("connected", || bridge.connected());
        assert!(bridge.selection_stale());
        // Nothing is sent to whatever tab 9 is now, and its reports are not
        // taken for the music.
        assert!(bridge.pause(TabId(9)).is_err());
        say(
            &mut client,
            r#"{"type":"source_state","tabId":9,"playing":true,"volume":0.5}"#,
        );
        say(&mut client, r#"{"type":"tabs","tabs":[{"id":9}]}"#);
        wait_until("tabs", || bridge.tabs().len() == 1);
        assert!(bridge.playback_state(TabId(9)).is_err());
    }

    #[test]
    fn picking_the_tab_again_in_the_new_session_selects_it() {
        let mut bridge = start();
        bridge.select(Some(TabId(9)), Some("run-1"));
        let (mut client, _) = paired_in_session(&bridge, "run-2");
        wait_until("connected", || bridge.connected());

        bridge.select(Some(TabId(4)), Some("run-2"));

        assert_eq!(
            hear(&mut client),
            serde_json::json!({"type":"select_source","tabId":4})
        );
        assert!(!bridge.selection_stale());
    }

    #[test]
    fn a_tab_saved_without_a_session_is_still_selected() {
        let mut bridge = start();
        bridge.select(Some(TabId(9)), None);

        let (_client, selection) = paired_in_session(&bridge, "run-2");

        assert_eq!(selection["tabId"], 9);
        assert!(!bridge.selection_stale());
    }

    #[test]
    fn a_tab_with_a_session_is_stale_for_an_extension_that_reports_none() {
        let mut bridge = start();
        bridge.select(Some(TabId(9)), Some("run-1"));

        let _client = paired(&bridge);
        wait_until("connected", || bridge.connected());

        assert!(bridge.selection_stale());
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
