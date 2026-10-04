//! The JSON messages exchanged with the browser extension (SPEC.md 6).
//!
//! Every message is an object with a `type` field. Keep this file and
//! `extension/src/protocol.ts` in step.

use cricket_core::source::{TabId, TabInfo};
use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u32 = 1;

/// Extension to app.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    /// Must be the first message on a connection.
    Hello {
        version: u32,
        token: String,
        /// Identifies this browser run. Tab ids are only meaningful within
        /// one, so a saved tab is matched to it. Absent from extensions
        /// older than this field.
        #[serde(default)]
        session: Option<String>,
        /// Executable name of the browser, for example `chrome.exe`.
        #[serde(default)]
        browser: Option<String>,
    },
    /// The full list of open tabs, sent whenever it changes.
    Tabs { tabs: Vec<TabInfo> },
    /// State of the tab selected as the music source, sent regularly.
    SourceState {
        #[serde(rename = "tabId")]
        tab_id: TabId,
        playing: bool,
        /// Volume of the tab's media, 0.0 to 1.0. Absent if the page has no
        /// media the extension can control.
        #[serde(default)]
        volume: Option<f32>,
    },
    /// Keeps the extension's service worker and the connection alive.
    Ping,
}

/// App to extension.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    /// Answer to `hello`. After `ok: false` the app closes the connection.
    HelloAck {
        ok: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    /// Which tab is the music source; `null` for none.
    SelectSource {
        #[serde(rename = "tabId")]
        tab_id: Option<TabId>,
    },
    Command {
        cmd: CommandKind,
        #[serde(rename = "tabId")]
        tab_id: TabId,
        #[serde(skip_serializing_if = "Option::is_none")]
        volume: Option<f32>,
        /// Reserved: the app drives fades itself, one volume step at a time.
        #[serde(rename = "fadeMs", skip_serializing_if = "Option::is_none")]
        fade_ms: Option<u32>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandKind {
    Pause,
    Resume,
    SetVolume,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hello() {
        let message: ClientMessage =
            serde_json::from_str(r#"{"type":"hello","version":1,"token":"abc"}"#).unwrap();
        assert_eq!(
            message,
            ClientMessage::Hello {
                version: 1,
                token: "abc".to_string(),
                session: None,
                browser: None
            }
        );
    }

    #[test]
    fn parses_hello_with_a_session() {
        let message: ClientMessage =
            serde_json::from_str(r#"{"type":"hello","version":1,"token":"abc","session":"run-1"}"#)
                .unwrap();
        assert_eq!(
            message,
            ClientMessage::Hello {
                version: 1,
                token: "abc".to_string(),
                session: Some("run-1".to_string()),
                browser: None
            }
        );
    }

    #[test]
    fn parses_tabs_with_extra_fields() {
        let message: ClientMessage = serde_json::from_str(
            r#"{"type":"tabs","tabs":[{"id":5,"title":"Mix","url":"https://x","audible":true,"favIconUrl":"y"}]}"#,
        )
        .unwrap();
        assert_eq!(
            message,
            ClientMessage::Tabs {
                tabs: vec![TabInfo {
                    id: TabId(5),
                    title: "Mix".to_string(),
                    url: "https://x".to_string(),
                    audible: true
                }]
            }
        );
    }

    #[test]
    fn parses_source_state_with_and_without_volume() {
        let with: ClientMessage = serde_json::from_str(
            r#"{"type":"source_state","tabId":5,"playing":true,"volume":0.5}"#,
        )
        .unwrap();
        let without: ClientMessage =
            serde_json::from_str(r#"{"type":"source_state","tabId":5,"playing":false}"#).unwrap();

        assert_eq!(
            with,
            ClientMessage::SourceState {
                tab_id: TabId(5),
                playing: true,
                volume: Some(0.5)
            }
        );
        assert_eq!(
            without,
            ClientMessage::SourceState {
                tab_id: TabId(5),
                playing: false,
                volume: None
            }
        );
    }

    #[test]
    fn unknown_message_types_do_not_parse() {
        assert!(serde_json::from_str::<ClientMessage>(r#"{"type":"nope"}"#).is_err());
    }

    #[test]
    fn serializes_commands_in_the_spec_shape() {
        let pause = ServerMessage::Command {
            cmd: CommandKind::Pause,
            tab_id: TabId(5),
            volume: None,
            fade_ms: None,
        };
        let volume = ServerMessage::Command {
            cmd: CommandKind::SetVolume,
            tab_id: TabId(5),
            volume: Some(0.25),
            fade_ms: None,
        };

        assert_eq!(
            serde_json::to_string(&pause).unwrap(),
            r#"{"type":"command","cmd":"pause","tabId":5}"#
        );
        assert_eq!(
            serde_json::to_string(&volume).unwrap(),
            r#"{"type":"command","cmd":"set_volume","tabId":5,"volume":0.25}"#
        );
    }

    #[test]
    fn serializes_select_source_and_ack() {
        assert_eq!(
            serde_json::to_string(&ServerMessage::SelectSource { tab_id: None }).unwrap(),
            r#"{"type":"select_source","tabId":null}"#
        );
        assert_eq!(
            serde_json::to_string(&ServerMessage::HelloAck {
                ok: true,
                error: None
            })
            .unwrap(),
            r#"{"type":"hello_ack","ok":true}"#
        );
    }
}
