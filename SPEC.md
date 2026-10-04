# Cricket: Product Spec

## 1. Vision

A small desktop app that keeps your background music playing only while everything else is quiet. When any other sound starts, or the microphone is in use, the music fades out and pauses. After a stretch of silence, it fades back in and resumes.

The music source can be a desktop app or a specific Chrome tab, picked from one list.

## 2. Purpose

The user should never have to pause or resume music by hand for meetings, videos, games, or long notifications, and should never have music clashing with audio they need to hear.

## 3. How it differs from AutoGoose

| | AutoGoose | Cricket |
|---|---|---|
| Platforms | Windows | Windows, macOS |
| Mechanism | Lowers volume | Fades and pauses, then resumes |
| Music source | A music player app | An app or one specific Chrome tab |
| Other Chrome tabs as trigger | No (Chrome is one audio session) | Yes, via the extension |

## 4. Scenarios (confirmed behavior)

1. **Chrome tab as music, desktop app as trigger.** YouTube Music tab is the source and is playing. Zoom starts making sound, so music fades out and pauses. After the call ends and silence lasts the cooldown, music resumes from where it stopped.
2. **Chrome tab as music, another tab as trigger.** Music plays in tab A. A video starts in tab B, so music pauses. When tab B goes silent, music resumes.
3. **App as music (e.g. Spotify desktop), anything as trigger.** A game, a video, or a microphone in use pauses Spotify. When all is quiet, it resumes.
4. **Short sounds.** A notification shorter than the trigger delay does not pause the music. After a trigger ends, music waits for the resume cooldown of continuous silence before resuming.
5. **User pauses manually.** If the user pauses the music source themselves, Cricket does not restart it. Cricket only resumes what Cricket paused.

## 5. Behavior details

### 5.1 Activity detection

"Other sound" means any audio session other than the music source (and other than Cricket itself) whose peak level is above the threshold. Optionally, microphone use also counts.

- Desktop app sources: all other sessions count, including Chrome.
- Chrome tab source: the Chrome process is ignored by the desktop side. The extension reports other audible tabs instead.
- System sounds: ignored by default (setting).

### 5.2 State machine

States: `Idle` (no source or disabled), `Playing`, `FadingOut`, `PausedByCricket`, `FadingIn`, `PausedByUser`.

Transitions:

- `Playing` to `FadingOut`: other activity continuously present for at least the trigger delay.
- `FadingOut` to `PausedByCricket`: fade finished, pause command sent.
- `PausedByCricket` to `FadingIn`: no activity for at least the resume cooldown. Resume command sent, volume ramps up.
- `FadingIn` to `Playing`: fade finished.
- `FadingOut` or `FadingIn` back to the opposite direction if activity changes mid-fade (reverse smoothly from current volume, no jumps). The change must last the trigger delay before the fade reverses, so short gaps or blips do not make the volume wobble.
- Any state to `PausedByUser`: the source stops by itself without a Cricket command. Return to `Playing` only when the source starts playing again.
- Source not available (browser tab source whose extension is disconnected, or whose tab is closed or not reporting): the engine holds its state and sends no commands. Nothing is concluded from the missing reports, and the resume cooldown restarts once the source is back. A resume that was unconfirmed when the link dropped falls back to `PausedByCricket`.
- Restore the source's original volume after every resume.
- Never leave the source muted. The OS remembers per-app volume, so the volume is put back to the original as soon as the pause has taken effect, and is dropped to zero again only at the moment of resuming, just before the fade in. If Cricket exits or the user resumes by hand while paused, the music is audible.

### 5.3 Settings (with defaults)

| Setting | Default | Meaning |
|---|---|---|
| Trigger delay | 500 ms | How long other sound must last before pausing |
| Resume cooldown | 3000 ms | Continuous silence required before resuming |
| Fade out | 2500 ms | Fade to zero before pause (0 means pause immediately) |
| Fade in | 1500 ms | Fade up after resume |
| Sound threshold | peak 0.02 (about -34 dBFS) | Below this counts as silence |
| Treat microphone use as activity | On | Active capture session counts as activity |
| Ignore system sounds | On | Windows notification sounds do not trigger |
| Start with Windows / minimize to tray | Off / On | Standard app behavior ("start with Windows" arrives in Phase 6) |

### 5.4 UI

- Main window: source picker (apps and Chrome tabs in one list, with live "making sound" indicators), on/off switch, status line showing the current state.
- Settings panel: the table above. A collapsed "mini mode" showing only status and the on/off switch.
- Tray icon: shows state, quick on/off.
- A visible diagnostic view (can be hidden behind a toggle) listing detected sessions and peak levels. This is for debugging and for the user to see why music paused.

## 6. Chrome extension

- Manifest V3. Connects to the desktop app over WebSocket on 127.0.0.1 using a pairing token.
- Page scripts run only on a default list of music sites (the manifest's `content_scripts`) and on sites the user allows from the popup (`optional_host_permissions` plus a registered content script). Tabs on other sites still count as activity through the browser's audible flag but cannot be the music source; the tab list marks them `controllable: false`.
- Reports the list of tabs (id, title, audible) whenever it changes. Tab addresses are not sent.
- Receives commands: select music tab, pause, resume, fade volume.
- Pause and resume through an injected script on the music tab, handling media elements and fading `element.volume`.
- Known risk: some sites do not respond cleanly to a direct pause. Test early with the sites the user actually uses (YouTube, YouTube Music, Spotify Web).

Protocol (JSON messages, versioned):

- App to extension, first: `challenge {version, nonce}`
- Extension to app: `hello {version, clientNonce, proof, session?, browser?}`, `tabs {tabs:[{id,title,audible}]}`, `source_state {tabId, playing, volume?}`, `ping {}`
- App to extension: `hello_ack {ok, error?, proof?}`, `select_source {tabId|null}`, `command {cmd:"pause"|"resume"|"set_volume", tabId, volume?, fadeMs?}`

Notes on the protocol as built:

- Every message carries a `type` field with the name shown above. The app speaks first with `challenge`; `hello` must come next. Anything else, a wrong proof or an unsupported version gets `hello_ack {ok:false}` and the connection is closed.
- Pairing (protocol 2): the token is never sent. `proof` is hex HMAC-SHA256 keyed with the token over `"cricket-v2-client:" + serverNonce + ":" + clientNonce`; the app's `hello_ack.proof` is the same over `"cricket-v2-server:"...`. Each side checks the other's proof, so the extension does not take commands from something else on the port. Protocol 1 and 2 are not compatible; update app and extension together.
- The app only accepts connections whose `Origin` is exactly `chrome-extension://<Cricket's extension id>` (the manifest carries a fixed `key`, so the id is the same everywhere). Web pages and other extensions are refused before any message is read. Messages are capped at 1 MiB, `hello` at 4 KiB, and at most 8 connections are open at once.
- The pairing token can be replaced from the app ("Generate a new token"); the extension is disconnected and needs the new token.
- `browser` is the executable hosting the tabs (default `chrome.exe`); the desktop side ignores that process while a tab is the source.
- `source_state.volume` is absent when the page has no media the extension can reach; the app then pauses without fading.
- The app drives fades itself with repeated `set_volume` commands, so they can reverse mid-way. `fadeMs` is reserved.
- A tab id only lives as long as the browser run. The extension sends a `session` id (random, kept in `chrome.storage.session`, so it survives the service worker stopping but not a browser restart). A saved tab source remembers the session it was picked in; if the connected session differs, the tab is not selected, the engine holds, and the UI asks to pick the tab again.

## 7. Architecture

See `CLAUDE.md` for layout and rules. Key points:

- `cricket-core`: engine state machine, settings, traits `AudioBackend` (list sessions and peaks, mic state, get and set session volume) and `MediaController` (play, pause, playing state).
- `cricket-audio-win`: WASAPI sessions, peak meters, session volume, SMTC.
- `cricket-bridge`: WebSocket server, token pairing, message types.
- `src-tauri`: wires backends, bridge, and UI together. Exposes Tauri commands for the UI.
- macOS later: Swift helper using Core Audio process taps (macOS 14.2+), same traits.

## 8. Non-goals for v1

- Lowering volume to a level instead of pausing (possible later as a second mode).
- Multiple fallback music sources.
- Mobile, Linux, Firefox, Safari.
- Cloud sync or accounts.

## 9. Assumptions (defaults I chose; change if wrong)

- Microphone in use counts as activity, even while the user is silent in a call.
- Fade then pause, not pause instantly.
- One music source at a time.
- Stack is Tauri v2 with a Rust core, so the same UI and core serve both platforms.

## 10. Phases

Each phase ends with a manual test checklist for the user. Do not start the next phase before confirmation.

### Phase 0: Scaffold
- [x] Verify toolchain: Rust (MSVC), Node, pnpm, WebView2.
- [x] Cargo workspace with the crates in the layout, Tauri v2 app with React + TS + Vite.
- [x] fmt, clippy, tests wired; basic CI (GitHub Actions, Windows runner).
- [x] README with setup steps. CLAUDE.md commands section filled in.

### Phase 1: Windows audio probe
- [x] `AudioBackend` and `MediaController` traits in core.
- [x] Windows backend: enumerate sessions grouped by app, peak levels, mic capture state, session volume get and set, SMTC pause and resume.
- [x] A debug CLI that prints live sessions and peaks, and can pause and resume a chosen app and fade its volume.
- [x] Manual test: user runs it with Spotify, a game or video, Zoom or any mic app, and confirms the readings make sense.

### Phase 2: Engine
- [x] Pure state machine with fake clock; unit tests for every scenario in section 4 and every transition in 5.2, including mid-fade reversal and the user-pause rule.
- [x] Wire engine to the Windows backend in a headless run mode with detailed logs.
- [x] Manual test: scenario 3 end to end (app source) in the headless mode.

### Phase 3: UI
- [x] Source picker, on/off, status, settings panel, tray, mini mode, persistence, diagnostics view.
- [x] Manual test: scenarios 3, 4, 5 through the real UI.

### Phase 4: Chrome extension
- [x] Bridge crate, pairing flow, extension (tabs list, select, pause, resume, fade).
- [x] Source picker shows Chrome tabs; scenarios 1 and 2 work.
- [x] Manual test on the user's real music sites.

### Phase 5: macOS
- [ ] Core Audio process tap helper, `AudioBackend` and `MediaController` for macOS, permissions flow, signing and notarization.

### Phase 6: Polish and release
- [ ] Autostart, installer (Windows), update story, icon and mascot, README and screenshots.
