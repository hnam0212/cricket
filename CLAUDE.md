# Cricket

Cricket is a desktop app (Windows first, macOS later) that keeps your background music playing while the machine is quiet and pauses it automatically when anything else makes sound or the microphone is in use. It resumes after a cooldown of silence.

The music source can be a desktop app (e.g. Spotify) or one specific tab in Chrome. Pausing and resuming uses fades.

Think "AutoGoose, but play/pause instead of ducking, cross-platform, with Chrome tab support".

Full product spec: `SPEC.md`. Read it before starting any phase.

## Stack

- App shell: Tauri v2 (Rust core + web UI)
- UI: React + TypeScript + Vite, pnpm
- Core logic: Rust workspace
- Windows audio: `windows` crate (WASAPI + Windows.Media.Control / SMTC)
- macOS audio (later): Swift helper using Core Audio process taps
- Chrome extension: Manifest V3, plain TypeScript
- Extension <-> app bridge: WebSocket on 127.0.0.1 with a pairing token

## Repo layout (target)

```
cricket/
  CLAUDE.md
  SPEC.md
  Cargo.toml                 # workspace
  crates/
    cricket-core/            # platform-agnostic: engine, settings, traits
    cricket-audio-win/       # Windows implementation of the traits
    cricket-audio-mac/       # later
    cricket-bridge/          # WebSocket server for the extension
  src-tauri/                 # Tauri app, wires everything together
  ui/                        # React frontend
  extension/                 # Chrome MV3 extension
```

## Commands

Keep these current. Run them from the repo root.

- Install JS deps: `pnpm install`
- Dev: `pnpm tauri dev` (Vite on `http://127.0.0.1:1420`, then the app window)
- Release build: `pnpm tauri build`
- Tests: `cargo test --workspace`
- Lint: `cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --check`
- UI checks: `pnpm -C ui lint` and `pnpm -C ui typecheck`
- UI build: `pnpm -C ui build`
- Audio probe (debug CLI): `cargo run -p cricket-audio-win --bin cricket-probe -- <command>`. Commands: `list`, `watch`, `status <app>`, `pause <app>` (alias `fade <app>`: fade to silence, then pause), `resume <app>` (resume, then fade in), `volume <app> [level]`, `cycle <app>`. Run it with no command for full usage.

The Tauri crate embeds `ui/dist` at compile time. On a fresh clone, run `pnpm -C ui build` before any cargo command that compiles `src-tauri`.

TypeScript is pinned to 6.x because typescript-eslint does not support TypeScript 7 yet.

## Architecture rules

1. Platform-agnostic logic lives in `cricket-core`. OS-specific code only appears behind the traits `AudioBackend` and `MediaController`. `cricket-core` must compile and test without any OS audio API.
2. The ducking engine is a pure state machine. Time comes from an injected `Clock`, input comes from injected activity snapshots, output is a list of commands. No I/O inside the engine. It must have thorough unit tests using a fake clock.
3. Ownership rule: Cricket only resumes music that Cricket paused. If the user pauses the music source manually, Cricket must not restart it.
4. Cricket must exclude its own process from activity detection.
5. When the music source is a Chrome tab, the desktop side ignores the whole Chrome process for activity detection (it contains the music itself). Other tabs are reported by the extension instead.
6. Audio polling runs on a dedicated thread, roughly every 50 ms, and never blocks on UI or network.
7. Settings are stored as versioned JSON in the OS app config dir. Unknown or missing fields fall back to defaults.
8. No telemetry. The only network listener is the WebSocket on 127.0.0.1, protected by a pairing token.

## Working agreement

- Work on one phase at a time (see `SPEC.md`). At the end of a phase, stop and give: a short summary, what changed, and a manual test checklist. Do not start the next phase until the user confirms.
- You cannot hear audio. Never claim audio behavior is verified. Instead add diagnostic logging (session list, peak levels, state transitions with timestamps) and give the user concrete steps to test on their machine and paste logs back.
- Ask before adding a significant new dependency.
- Small, focused commits with conventional commit messages (`feat:`, `fix:`, `refactor:`, `test:`, `docs:`).
- The user develops on native Windows (PowerShell). Do not build the app inside WSL.
- Keep UI text in English for now, but route all strings through one place so they can be localized later.
- Prefer boring, readable code over clever code. Comment the why, not the what.

## Windows audio gotchas

- Initialize COM on every thread that touches WASAPI (`CoInitializeEx`).
- Audio sessions appear and disappear dynamically. Register `IAudioSessionNotification` instead of enumerating once.
- One app can own several sessions (Chrome has many). Group sessions by executable name or process tree.
- Use the per-session peak meter (`IAudioMeterInformation`) for "is this app making sound", not the endpoint master peak.
- Microphone use: look at capture endpoints; an Active capture session means the mic is in use.
- Pause and resume apps through SMTC (`GlobalSystemMediaTransportControlsSessionManager`) where possible. Fall back to media keys only if SMTC does not expose the app.
- Fade by changing session volume (`ISimpleAudioVolume`) and restore the user's original volume after resuming.

## Chrome extension gotchas

- MV3 service workers can be suspended. Keep the WebSocket alive with periodic messages and reconnect with backoff.
- `tab.audible` is the signal for "this tab is making sound". Exclude the selected music tab.
- Pausing a tab: inject a script that pauses and resumes the page's media elements and fades `element.volume`. Some sites (Spotify Web) may keep their own UI state out of sync; test early on the sites the user actually uses.

## Definition of done (any phase)

- Builds cleanly, tests pass, clippy and fmt clean.
- New behavior has tests where the logic is testable without hardware.
- `CLAUDE.md` commands section and `SPEC.md` phase checklist updated.
- Manual test checklist delivered to the user.
