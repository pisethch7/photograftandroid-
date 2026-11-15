//! The desktop model call, and where its provider settings live.
//!
//! `photocraft-ai` builds the request, sends the same bytes everywhere and validates the answer;
//! this module supplies the one thing it deliberately does not have: a socket. `ureq` is already
//! in the graph (through `craft-update`), so a request is one blocking POST on the AI worker
//! thread, with rustls and the platform's certificate store — no async runtime, and no second TLS
//! stack to keep patched.
//!
//! The settings — base URL, model and key — live in `ai.json` in the app's config directory,
//! beside `preferences.json`. They are never a command parameter and never in `ui.ron`: either
//! would be journaled, replayed and readable through the control channel, and a key is none of
//! those things. The file is created `0600` on Unix for the same reason.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use photocraft_ai::http::{one_line, refusal};
use photocraft_ai::{Error, OpenAi, Planner, Settings, Transport};
use photocraft_ui_egui::ai_ui::Service;

/// Longest a single request may take, start to finish. A reasoning model on a long prompt is
/// slow; a dead socket must still end, and the panel must say so.
const TIMEOUT: Duration = Duration::from_secs(120);
/// Longest answer body we will read. A brief is about a kilobyte; anything larger is not one, and
/// a server that sends megabytes must not be copied into memory whole.
const MAX_BODY: u64 = 1 << 20;

/// Window › AI › Create Poster's service: the shared settings handle, the planner factory and the
/// one place the settings file is written.
pub fn service() -> Service {
    Service { settings: Arc::new(Mutex::new(load())), make: Arc::new(make), save: Box::new(save) }
}

/// Build the planner a request uses, from the settings as they are when it starts.
///
/// Called on the UI thread before the worker starts, so a base URL or model that cannot be used is
/// an immediate message in the panel rather than a failed request a person waits two minutes for.
fn make(settings: &Settings) -> Result<Box<dyn Planner + Send>, String> {
    let endpoint = settings.endpoint().map_err(|error| error.to_string())?;
    let key = settings.key().map_err(|error| error.to_string())?;
    let planner: Box<dyn Planner + Send> = Box::new(OpenAi::new(endpoint, key, Ureq));
    Ok(planner)
}

/// The desktop transport: one blocking POST through `ureq`.
///
/// The agent is built per request, so a settings change (a new host, a new certificate) is not
/// answered from a pooled connection opened for the last one.
struct Ureq;

impl Transport for Ureq {
    fn post_json(&self, url: &str, headers: &[(&str, &str)], body: &str) -> photocraft_ai::Result<String> {
        let agent = ureq::Agent::new_with_config(
            ureq::Agent::config_builder()
                // Read the server's own error document instead of a bare status: "Incorrect API
                // key provided" is a sentence a person can act on, "400" is not.
                .http_status_as_error(false)
                .timeout_global(Some(TIMEOUT))
                .build(),
        );
        let mut request = agent.post(url);
        for (name, value) in headers {
            // Headers can carry the key: they are never logged, here or in `ureq`.
            request = request.header(*name, *value);
        }
        let mut response =
            request.send(body).map_err(|error| Error::Provider(format!("Could not reach the model server: {}", one_line(&error.to_string()))))?;
        let status = response.status().as_u16();
        let text = response
            .body_mut()
            .with_config()
            .limit(MAX_BODY)
            .read_to_string()
            .map_err(|error| Error::Provider(format!("The model server's answer could not be read: {}", one_line(&error.to_string()))))?;
        if (200..300).contains(&status) { Ok(text) } else { Err(Error::Provider(refusal(status, &text))) }
    }
}

fn settings_file() -> Option<PathBuf> {
    crate::app_dirs::config_dir().map(|dir| dir.join("ai.json"))
}

/// The saved settings, or the default ones when nothing was saved.
fn load() -> Settings {
    let Some(path) = settings_file() else { return Settings::default() };
    match load_from(&path) {
        Ok(settings) => settings,
        Err(error) => {
            // A broken file must not stop the app or throw the settings away silently: the panel
            // shows "not set up", and saving again overwrites the file.
            log::warn!("AI settings: {error}");
            Settings::default()
        }
    }
}

fn load_from(path: &Path) -> Result<Settings, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => Settings::from_json(&text).map_err(|error| error.to_string()),
        // No file is a fresh install, not a mistake.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Settings::default()),
        Err(error) => Err(format!("Could not read {}: {error}", path.display())),
    }
}

fn save(text: &str) -> Result<(), String> {
    let path = settings_file().ok_or("This build has no settings folder to keep the AI settings in")?;
    save_to(&path, text)
}

fn save_to(path: &Path, text: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| format!("Could not create {}: {error}", parent.display()))?;
    }
    // Through the same atomic write as preferences: a failed save never truncates the old file.
    photocraft_format::atomic_write(path, text.as_bytes()).map_err(|error| format!("Could not write {}: {error}", path.display()))?;
    restrict(path);
    Ok(())
}

/// The file holds an API key: on Unix only its owner may read it. Windows inherits the profile
/// folder's per-user ACL, which is already narrower than a world-readable file.
#[cfg(unix)]
fn restrict(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Err(error) = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)) {
        log::warn!("Could not restrict {} to its owner: {error}", path.display());
    }
}

#[cfg(not(unix))]
fn restrict(_path: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> Settings {
        Settings { base_url: "https://api.example.test/v1".into(), model: "gpt-4o-mini".into(), key: Some("sk-test-key".into()) }
    }

    #[test]
    fn settings_round_trip_and_the_file_is_private() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let path = dir.path().join("ai.json");
        save_to(&path, &settings().to_json().expect("JSON")).expect("saved");
        assert_eq!(load_from(&path).expect("loaded"), settings());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).expect("metadata").permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "the key is readable by its owner only");
        }
    }

    #[test]
    fn a_missing_file_is_a_fresh_install_and_a_broken_one_is_reported() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let missing = dir.path().join("ai.json");
        assert_eq!(load_from(&missing).expect("a fresh install"), Settings::default());
        let broken = dir.path().join("broken.json");
        std::fs::write(&broken, "{not json}").expect("written");
        assert!(load_from(&broken).is_err());
        // Saving over it repairs it, which is what the panel's Save does.
        save_to(&broken, &Settings::default().to_json().expect("JSON")).expect("saved");
        assert_eq!(load_from(&broken).expect("loaded"), Settings::default());
    }

    #[test]
    fn a_planner_is_built_from_the_settings_and_describes_the_host() {
        let planner = make(&settings()).expect("a planner");
        assert_eq!(planner.describe(), "api.example.test/gpt-4o-mini");
        assert!(!planner.describe().contains("sk-test"), "never the key");
        // A local server with no key is a planner too.
        let local = Settings { base_url: "http://127.0.0.1:11434".into(), model: "llama3".into(), key: None };
        assert_eq!(make(&local).expect("a planner").describe(), "127.0.0.1:11434/llama3");
        // Settings that cannot be used are refused before a worker starts.
        let mut bad = settings();
        bad.base_url = "http://api.example.test/v1".into();
        assert!(make(&bad).is_err());
        assert!(make(&Settings::default()).is_err());
    }

    #[test]
    fn a_transport_failure_is_one_line_and_never_echoes_the_key() {
        // No network: an unusable URL fails before a socket is opened.
        let error = Ureq.post_json("https://", &[("Authorization", "Bearer sk-test-key")], "{}").expect_err("an unusable URL");
        let message = error.to_string();
        assert!(!message.is_empty());
        assert!(!message.contains("sk-test-key"), "{message}");
        assert!(!message.contains('\n'), "{message}");
        assert!(message.len() <= photocraft_ai::http::MAX_REASON + 80, "a panel line, not a page: {message}");
    }
}
