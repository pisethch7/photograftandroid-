//! Where the endpoint and the key live.
//!
//! Never in a command parameter. Parameters are journaled, replayed, and readable through the
//! control channel and the UI state file; a key is none of those things. The app keeps this in
//! its own file, outside the document, the journal and `ui.ron`, and hands the panel a shared
//! handle so an edit reaches the next request without passing through a command.
//!
//! `Debug` is redacted, so a settings value can be logged or printed in a test failure without
//! printing the key. The key is stored as written: v1 is a plain file in the app's config
//! directory (readable by whoever can read that directory), and the Android build will move it to
//! the Keystore. That is stated in the panel, not hidden.

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::http::{Endpoint, Key};

/// The provider settings, as the panel edits them.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// The base URL of an OpenAI-compatible endpoint, for example `https://api.openai.com/v1`
    /// or `http://127.0.0.1:11434`.
    pub base_url: String,
    /// The model name, for example `gpt-4o-mini` or `llama3`.
    pub model: String,
    /// The API key, absent for a local server that wants none.
    pub key: Option<String>,
}

impl std::fmt::Debug for Settings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Settings").field("base_url", &self.base_url).field("model", &self.model).field("key", &self.key.as_ref().map(|_| "****")).finish()
    }
}

impl Settings {
    /// The endpoint these settings name, or what is wrong with them.
    pub fn endpoint(&self) -> Result<Endpoint> {
        Endpoint::new(&self.base_url, &self.model)
    }

    /// The key, when one is set. A blank key is no key, not an error: a local server wants none.
    pub fn key(&self) -> Result<Option<Key>> {
        match self.key.as_deref().map(str::trim).filter(|key| !key.is_empty()) {
            Some(key) => Ok(Some(Key::new(key)?)),
            None => Ok(None),
        }
    }

    /// Everything the first request needs, checked.
    pub fn validate(&self) -> Result<()> {
        self.endpoint()?;
        self.key()?;
        Ok(())
    }

    /// Whether a base URL and a model are both set (the key may be absent).
    pub fn configured(&self) -> bool {
        !self.base_url.trim().is_empty() && !self.model.trim().is_empty()
    }

    /// The settings as the file holds them.
    pub fn to_json(&self) -> Result<String> {
        serde_json::to_string_pretty(self).map_err(|error| Error::Provider(format!("Could not write the AI settings: {error}")))
    }

    /// Read settings. An empty file is a fresh install, not a mistake.
    pub fn from_json(text: &str) -> Result<Self> {
        if text.trim().is_empty() {
            return Ok(Self::default());
        }
        serde_json::from_str(text).map_err(|error| Error::Provider(format!("The AI settings file is not valid: {error}")))
    }

    /// What the panel shows beside the endpoint: `api.openai.com/gpt-4o-mini`, or `not set up`.
    pub fn describe(&self) -> String {
        match self.endpoint() {
            Ok(endpoint) => format!("{}/{}", endpoint.host(), endpoint.model()),
            Err(_) => "not set up".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> Settings {
        Settings { base_url: " https://api.openai.com/v1 ".into(), model: " gpt-4o-mini ".into(), key: Some(" sk-test-key ".into()) }
    }

    #[test]
    fn the_key_is_never_printed() {
        let debug = format!("{:?}", settings());
        assert!(!debug.contains("sk-test-key"), "{debug}");
        assert!(debug.contains("gpt-4o-mini"));
        assert!(debug.contains("****"));
    }

    #[test]
    fn settings_round_trip_through_the_file() {
        let text = settings().to_json().expect("JSON");
        let back = Settings::from_json(&text).expect("settings");
        assert_eq!(back, settings());
        assert_eq!(Settings::from_json("").expect("a fresh install"), Settings::default());
        assert_eq!(Settings::from_json("   \n").expect("a fresh install"), Settings::default());
        assert!(Settings::from_json("{not json}").is_err());
        // A file written by a later build with more fields still loads.
        let later = Settings::from_json(r#"{"base_url": "https://x.test/v1", "model": "m", "key": null, "temperature": 1}"#).expect("forward compatible");
        assert_eq!(later.model, "m");
    }

    #[test]
    fn a_blank_key_is_no_key_and_a_bad_one_is_an_error() {
        let mut s = settings();
        s.key = Some("   ".into());
        assert!(s.key().expect("no key").is_none());
        s.key = None;
        assert!(s.key().expect("no key").is_none());
        s.key = Some("sk-a b".into());
        assert!(s.key().is_err());
    }

    #[test]
    fn the_description_is_a_status_line() {
        assert_eq!(settings().describe(), "api.openai.com/gpt-4o-mini");
        assert_eq!(Settings::default().describe(), "not set up");
        assert!(!settings().describe().contains("sk-test"));
        assert!(settings().configured());
        assert!(!Settings::default().configured());
        assert!(!Settings { base_url: "https://x.test/v1".into(), ..Settings::default() }.configured());
    }

    #[test]
    fn validation_is_the_endpoints_and_the_keys() {
        assert!(settings().validate().is_ok());
        let mut s = settings();
        s.base_url = "http://api.openai.com/v1".into();
        assert!(s.validate().is_err(), "plain http to a remote host");
        let mut s = settings();
        s.model = String::new();
        assert!(s.validate().is_err());
    }
}
