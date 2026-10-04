# Cricket

Cricket keeps your background music playing while the machine is quiet, and fades it out and pauses it when anything else makes sound or the microphone is in use. After a stretch of silence it fades the music back in.

The music source can be a desktop app (such as Spotify) or one specific Chrome tab. Windows first, macOS later.

Status: Phase 3 (UI). The Windows app works for desktop-app music sources: pick the source, and Cricket pauses and resumes it. Chrome tab sources come next. See `SPEC.md` for the product spec and the phase plan, and `CLAUDE.md` for the architecture rules.

## Prerequisites (Windows)

- Git
- Visual Studio Build Tools 2022 with the "Desktop development with C++" workload
- Rust via rustup, MSVC toolchain (`stable-x86_64-pc-windows-msvc`)
- Node.js LTS
- pnpm
- WebView2 runtime (included in Windows 11)

Install with winget:

```powershell
winget install --id Microsoft.VisualStudio.2022.BuildTools -e --override "--wait --passive --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
winget install --id Rustlang.Rustup -e
rustup default stable-x86_64-pc-windows-msvc
winget install --id OpenJS.NodeJS.LTS -e
winget install --id pnpm.pnpm -e
```

Open a new terminal afterwards so the updated PATH is picked up.

## Setup

```powershell
git clone https://github.com/hnam0212/cricket
cd cricket
pnpm install
```

## Run

```powershell
pnpm tauri dev
```

This starts the Vite dev server on `http://127.0.0.1:1420`, builds the Rust app and opens the Cricket window. The first build takes a few minutes.

To build a release installer:

```powershell
pnpm tauri build
```

## Audio probe

`cricket-probe` is a debug CLI for the Windows audio backend. It shows what Cricket sees (audio sessions per app, peak levels, microphone use, media sessions) and can pause, resume and fade one app.

```powershell
cargo run -p cricket-audio-win --bin cricket-probe -- list
cargo run -p cricket-audio-win --bin cricket-probe -- watch
cargo run -p cricket-audio-win --bin cricket-probe -- cycle spotify.exe
cargo run -p cricket-audio-win --bin cricket-probe -- run spotify.exe
```

`run` is Cricket without the UI: it keeps the chosen app playing while the machine is quiet and pauses it when anything else makes sound. Run the probe without a command to see every command and option.

## Checks

```powershell
pnpm -C ui lint
pnpm -C ui typecheck
pnpm -C ui build
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

The Tauri crate embeds `ui/dist` at compile time, so run `pnpm -C ui build` once before `cargo clippy` or `cargo test` on a fresh clone. CI (`.github/workflows/ci.yml`) runs the same checks on a Windows runner.

## Layout

```
Cargo.toml              # Rust workspace
crates/
  cricket-core/         # platform-agnostic: engine, settings, traits
  cricket-audio-win/    # Windows implementation of the traits
  cricket-bridge/       # WebSocket server for the Chrome extension
src-tauri/              # Tauri app, wires everything together
ui/                     # React + TypeScript + Vite frontend
```
