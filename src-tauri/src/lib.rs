//! Tauri app: wires the core, the audio backend, the bridge and the UI
//! together. In Phase 0 it only proves the UI can call into `cricket-core`.

/// Smoke command for the scaffold: the UI shows this string to prove the
/// web side reaches the Rust core.
#[tauri::command]
fn core_info() -> String {
    cricket_core::describe()
}

pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![core_info])
        .run(tauri::generate_context!())
        .expect("error while running the Cricket app");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_info_comes_from_core() {
        assert_eq!(core_info(), cricket_core::describe());
    }
}
