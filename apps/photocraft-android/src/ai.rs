//! Window › AI › Create Poster's model call on a tablet.
//!
//! Android's own network stack does the POST. The Rust side writes `ai-request.json` and waits;
//! the Activity reads it, runs an `HttpURLConnection` on a background thread and writes
//! `ai-answer.json` (see `bridge.rs` and `PhotoCraftActivity`). That reuses the platform trust
//! store — no TLS to cross-compile, no second certificate bundle to keep current — needs no JNI
//! and adds no `unsafe`.
//!
//! The provider settings live in `ai.json` in the app's files directory, beside `prefs.json`.
//! Android's private storage is per-app, so the file needs no mode change here; the key never
//! reaches a command parameter, `ui.ron` or a document (see `photocraft_ai::settings`).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use photocraft_ai::http::{one_line, refusal};
use photocraft_ai::{Error, OpenAi, Planner, Settings, Transport};
use photocraft_ui_egui::ai_ui::Service;

use crate::bridge::{AiRequest, Bridge};

/// Longest one request may take before the panel is told it failed. The Activity's own read
/// timeout is shorter, so this is the backstop for an Activity that never answered at all.
const TIMEOUT: Duration = Duration::from_secs(130);
/// How often the worker looks for the Activity's answer. The Activity polls the bridge every
/// 150 ms, so a shorter wait here would only spin.
const POLL: Duration = Duration::from_millis(50);

/// Window › AI › Create Poster's service: settings in `ai.json`, and a transport through the
/// Activity.
pub fn service(bridge: Arc<Bridge>, files_dir: &Path) -> Service {
    let path = files_dir.join("ai.json");
    let settings = Arc::new(Mutex::new(load(&path)));
    let write = path.clone();
    let transport = bridge;
    Service {
        settings,
        make: Arc::new(move |settings: &Settings| {
            let endpoint = settings.endpoint().map_err(|error| error.to_string())?;
            let key = settings.key().map_err(|error| error.to_string())?;
            let planner: Box<dyn Planner + Send> = Box::new(OpenAi::new(endpoint, key, BridgeTransport { bridge: transport.clone() }));
            Ok(planner)
        }),
        save: Box::new(move |text: &str| save_to(&write, text)),
    }
}

/// The transport: hand the request to the Activity and wait for its answer.
///
/// Runs on the AI worker thread, so blocking here is the point. The answer is matched by id, so a
/// late answer to a cancelled request is dropped rather than shown against the next one.
struct BridgeTransport {
    bridge: Arc<Bridge>,
}

impl Transport for BridgeTransport {
    fn post_json(&self, url: &str, headers: &[(&str, &str)], body: &str) -> photocraft_ai::Result<String> {
        let id = self.bridge.next_id();
        let request = AiRequest {
            id,
            url: url.to_string(),
            headers: headers.iter().map(|(name, value)| ((*name).to_string(), (*value).to_string())).collect(),
            body: body.to_string(),
        };
        self.bridge.ai_request(&request).map_err(Error::Provider)?;
        let deadline = Instant::now() + TIMEOUT;
        loop {
            if let Some(answer) = self.bridge.take_ai_answer(id) {
                if let Some(error) = answer.error {
                    return Err(Error::Provider(one_line(&error)));
                }
                if (200..300).contains(&answer.status) {
                    return Ok(answer.body);
                }
                return Err(Error::Provider(refusal(answer.status, &answer.body)));
            }
            if Instant::now() >= deadline {
                return Err(Error::Provider("The model server did not answer in time".into()));
            }
            std::thread::sleep(POLL);
        }
    }
}

/// The saved settings, or the default ones when nothing was saved.
fn load(path: &Path) -> Settings {
    match load_from(path) {
        Ok(settings) => settings,
        Err(error) => {
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

fn save_to(path: &Path, text: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| format!("Could not create {}: {error}", parent.display()))?;
    }
    photocraft_format::atomic_write(path, text.as_bytes()).map_err(|error| format!("Could not write {}: {error}", path.display()))
}

/// The settings file, for a status line or a test.
pub fn settings_path(files_dir: &Path) -> PathBuf {
    files_dir.join("ai.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir() -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let d = std::env::temp_dir().join(format!("photocraft-ai-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn settings() -> Settings {
        Settings { base_url: "https://api.example.test/v1".into(), model: "gpt-4o-mini".into(), key: Some("sk-test-key".into()) }
    }

    #[test]
    fn settings_round_trip_and_a_broken_file_is_reported() {
        let d = dir();
        let path = settings_path(&d);
        assert_eq!(load_from(&path).expect("a fresh install"), Settings::default());
        save_to(&path, &settings().to_json().expect("JSON")).expect("saved");
        assert_eq!(load_from(&path).expect("loaded"), settings());
        std::fs::write(&path, "{not json}").expect("written");
        assert!(load_from(&path).is_err());
        // The panel's Save repairs it.
        save_to(&path, &Settings::default().to_json().expect("JSON")).expect("saved");
        assert_eq!(load_from(&path).expect("loaded"), Settings::default());
        std::fs::remove_dir_all(d).ok();
    }

    #[test]
    fn the_activitys_answer_becomes_a_body_or_a_sentence() {
        let d = dir();
        let bridge = Arc::new(Bridge::new(&d).unwrap());
        let transport = BridgeTransport { bridge: bridge.clone() };
        // The Activity answers the request it was given, as it would after a POST.
        let answer = |id: u64, json: &str| std::fs::write(bridge.root().join("ai-answer.json"), json.replace("ID", &id.to_string())).unwrap();
        let client = std::thread::spawn(move || transport.post_json("https://api.example.test/v1/chat/completions", &[("Authorization", "Bearer k")], "{}"));
        // Wait for the request file, then answer it. Reading it is what the Activity does, so it
        // deletes it too: a leftover would answer the next request's id.
        let request = loop {
            if let Ok(text) = std::fs::read_to_string(bridge.root().join("ai-request.json")) {
                let _ = std::fs::remove_file(bridge.root().join("ai-request.json"));
                break serde_json::from_str::<AiRequest>(&text).unwrap();
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        assert_eq!(request.url, "https://api.example.test/v1/chat/completions");
        assert_eq!(request.headers, vec![("Authorization".to_string(), "Bearer k".to_string())]);
        answer(request.id, r#"{"id": ID, "status": 200, "body": "{\"ok\":true}"}"#);
        assert_eq!(client.join().expect("the worker"), Ok("{\"ok\":true}".to_string()));
        std::fs::remove_dir_all(d).ok();
    }

    #[test]
    fn a_refusal_and_a_dead_connection_are_sentences() {
        let d = dir();
        let bridge = Arc::new(Bridge::new(&d).unwrap());
        let answer = |id: u64, json: &str| std::fs::write(bridge.root().join("ai-answer.json"), json.replace("ID", &id.to_string())).unwrap();
        let run = |json: String| {
            let transport = BridgeTransport { bridge: bridge.clone() };
            let client = std::thread::spawn(move || transport.post_json("https://api.example.test/v1", &[], "{}"));
            let request = loop {
                if let Ok(text) = std::fs::read_to_string(bridge.root().join("ai-request.json")) {
                    let _ = std::fs::remove_file(bridge.root().join("ai-request.json"));
                    break serde_json::from_str::<AiRequest>(&text).unwrap();
                }
                std::thread::sleep(Duration::from_millis(5));
            };
            answer(request.id, &json);
            client.join().expect("the worker").map_err(|error| error.to_string())
        };
        let error = run(r#"{"id": ID, "status": 401, "body": "{\"error\":{\"message\":\"Incorrect API key provided\"}}"}"#.into()).expect_err("refused");
        assert_eq!(error, "The model server refused (401): Incorrect API key provided");
        let error = run(r#"{"id": ID, "error": "no route to host\n(details)"}"#.into()).expect_err("no route");
        assert_eq!(error, "no route to host (details)", "one line, never a stack trace");
        std::fs::remove_dir_all(d).ok();
    }
}
