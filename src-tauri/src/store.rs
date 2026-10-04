//! The saved configuration file in the OS app config directory.

use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

use cricket_core::config::AppConfig;

pub struct ConfigStore {
    path: PathBuf,
    config: Mutex<AppConfig>,
}

impl ConfigStore {
    /// Loads the file, or starts from the defaults if it is missing or
    /// unreadable. A broken file is kept aside rather than overwritten, so
    /// the user's settings are not silently lost.
    pub fn load(path: PathBuf) -> Self {
        let config = match fs::read_to_string(&path) {
            Ok(text) => AppConfig::from_json(&text).unwrap_or_else(|error| {
                let backup = path.with_extension("json.broken");
                eprintln!(
                    "config: {} is not valid ({error}); using defaults, old file kept as {}",
                    path.display(),
                    backup.display()
                );
                let _ = fs::rename(&path, backup);
                AppConfig::default()
            }),
            Err(_) => AppConfig::default(),
        };
        Self {
            path,
            config: Mutex::new(config),
        }
    }

    pub fn get(&self) -> AppConfig {
        self.config.lock().expect("config lock").clone()
    }

    /// Changes the configuration and writes it to disk. Returns the result.
    pub fn update(&self, change: impl FnOnce(&mut AppConfig)) -> AppConfig {
        let mut config = self.config.lock().expect("config lock");
        change(&mut config);
        if let Err(error) = self.write(&config) {
            // Not fatal: the change still applies for this run.
            eprintln!("config: could not save {}: {error}", self.path.display());
        }
        config.clone()
    }

    fn write(&self, config: &AppConfig) -> std::io::Result<()> {
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir)?;
        }
        // Write next to the file and rename, so a crash mid-write cannot
        // leave a half-written config behind.
        let temp = self.path.with_extension("json.tmp");
        fs::write(&temp, config.to_json())?;
        fs::rename(&temp, &self.path)
    }
}
