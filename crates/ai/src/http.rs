//! The network seam: what the app does, and the little this crate checks before it asks.
//!
//! This crate never opens a socket, so it still compiles for `wasm32-unknown-unknown` and every
//! test runs offline. A [`Transport`] is implemented by the app: a Rust HTTP client on desktop,
//! Android's own stack through its Java bridge on a tablet, and [`Fake`] in tests. The request
//! and the checks are here, so all three paths send the same bytes and reject the same input.
//!
//! Three things are refused before anything is sent: a base URL that is not `http(s)`, a plain
//! `http` URL that is not on this machine (an API key must not travel in cleartext), and a URL
//! carrying credentials of its own. A key is never printed: [`Key`]'s `Debug` is redacted, and
//! nothing in this module formats one into a message.

use serde_json::Value;

use crate::error::{Error, Result};

/// Longest base URL we will accept.
pub const MAX_URL: usize = 2048;
/// Longest model name we will accept (some are a path: `accounts/…/models/…`).
pub const MAX_MODEL: usize = 200;
/// Longest API key we will accept.
pub const MAX_KEY: usize = 512;

/// A JSON POST, done by the app.
pub trait Transport: Send + Sync {
    /// POST `body` (JSON) to `url` with `headers`, and return a 2xx answer's body.
    ///
    /// `headers` can carry a secret, so an implementation must never log them. A non-2xx answer,
    /// a timeout, a DNS failure or an untrusted certificate is `Err` with a sentence a person can
    /// act on — the message reaches the panel.
    fn post_json(&self, url: &str, headers: &[(&str, &str)], body: &str) -> Result<String>;
}

/// One request as the transport saw it, for tests.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Seen {
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

/// A transport that answers from a script and keeps what it was asked.
///
/// Cheap to clone: every clone shares the script and the record, so a test can hand one copy to
/// the planner and read the requests from another.
#[derive(Clone, Default)]
pub struct Fake {
    inner: std::sync::Arc<std::sync::Mutex<State>>,
}

#[derive(Default)]
struct State {
    answer: String,
    failure: Option<String>,
    seen: Vec<Seen>,
}

impl Fake {
    /// Answer every request with `answer`, exactly as a server would send it.
    pub fn answering(answer: impl Into<String>) -> Self {
        let fake = Self::default();
        if let Ok(mut state) = fake.inner.lock() {
            state.answer = answer.into();
        }
        fake
    }

    /// Fail every request with `message`, as a refused connection would.
    pub fn failing(message: impl Into<String>) -> Self {
        let fake = Self::default();
        if let Ok(mut state) = fake.inner.lock() {
            state.failure = Some(message.into());
        }
        fake
    }

    /// Every request this transport was given, in order.
    pub fn seen(&self) -> Vec<Seen> {
        self.inner.lock().map(|state| state.seen.clone()).unwrap_or_default()
    }
}

impl Transport for Fake {
    fn post_json(&self, url: &str, headers: &[(&str, &str)], body: &str) -> Result<String> {
        // A poisoned lock is still readable state; treating it as fatal would turn a test's panic
        // into a second panic here.
        let mut state = self.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        state.seen.push(Seen {
            url: url.to_string(),
            headers: headers.iter().map(|(name, value)| ((*name).to_string(), (*value).to_string())).collect(),
            body: body.to_string(),
        });
        match &state.failure {
            Some(message) => Err(Error::Provider(message.clone())),
            None => Ok(state.answer.clone()),
        }
    }
}

/// Where the model is and what it is called, checked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Endpoint {
    base: String,
    model: String,
}

impl Endpoint {
    /// Check `base` and `model`, or say what is wrong with them.
    pub fn new(base: &str, model: &str) -> Result<Self> {
        let base = clean_base(base)?;
        let model = clean_model(model)?;
        Ok(Self { base, model })
    }

    /// The base URL as given, without a trailing slash.
    pub fn base(&self) -> &str {
        &self.base
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    /// The chat-completions URL this base names.
    ///
    /// A bare host gets `/v1/chat/completions` (OpenAI, Ollama, LM Studio); a base with a path
    /// gets `/chat/completions` appended (OpenAI's `…/v1`, Gemini's `…/v1beta/openai`, Azure's
    /// `…/openai/deployments/<name>`), and a base that already names the endpoint is left alone.
    pub fn chat_url(&self) -> String {
        let path = self.base.split_once("://").map(|(_, rest)| rest).unwrap_or_default();
        let path = path.split_once('/').map(|(_, path)| path).unwrap_or_default();
        if path.is_empty() {
            format!("{}/v1/chat/completions", self.base)
        } else if path == "chat/completions" || path.ends_with("/chat/completions") {
            self.base.clone()
        } else {
            format!("{}/chat/completions", self.base)
        }
    }

    /// Host and port, for a status line ("api.openai.com", "127.0.0.1:11434"). No credentials:
    /// a URL carrying them is refused.
    pub fn host(&self) -> &str {
        let rest = self.base.split_once("://").map(|(_, rest)| rest).unwrap_or_default();
        rest.split(['/', '?', '#']).next().unwrap_or_default()
    }
}

fn clean_base(raw: &str) -> Result<String> {
    let base = raw.trim();
    if base.is_empty() {
        return Err(Error::Provider("The base URL is empty: set one in the panel's provider settings".into()));
    }
    if base.chars().count() > MAX_URL {
        return Err(Error::Provider(format!("The base URL is longer than {MAX_URL} characters")));
    }
    if base.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err(Error::Provider("The base URL contains a space or control character".into()));
    }
    if base.contains('?') || base.contains('#') {
        return Err(Error::Provider("The base URL must not carry a query or fragment".into()));
    }
    let (rest, secure) = match (base.strip_prefix("https://"), base.strip_prefix("http://")) {
        (Some(rest), _) => (rest, true),
        (_, Some(rest)) => (rest, false),
        _ => return Err(Error::Provider("The base URL must start with https:// (or http:// for a server on this machine)".into())),
    };
    let authority = rest.split('/').next().unwrap_or_default();
    if authority.is_empty() {
        return Err(Error::Provider("The base URL names no host".into()));
    }
    if authority.contains('@') {
        // `user:key@host` would put a secret in the URL, where logs and error messages can show it.
        return Err(Error::Provider("The base URL must not carry a username or password: put the key in the key field".into()));
    }
    if !secure && !is_loopback(authority) {
        return Err(Error::Provider(format!(
            "http:// sends your key in cleartext: use https:// for {authority}, or a server on this machine (127.0.0.1, localhost)"
        )));
    }
    Ok(base.trim_end_matches('/').to_string())
}

fn clean_model(raw: &str) -> Result<String> {
    let model = raw.trim();
    if model.is_empty() {
        return Err(Error::Provider("No model is set: name one in the panel's provider settings".into()));
    }
    if model.chars().count() > MAX_MODEL {
        return Err(Error::Provider(format!("The model name is longer than {MAX_MODEL} characters")));
    }
    if model.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err(Error::Provider("The model name contains a space or control character".into()));
    }
    Ok(model.to_string())
}

/// `127.0.0.1`, `localhost` or `[::1]`, with an optional port.
fn is_loopback(authority: &str) -> bool {
    let host = if let Some(inner) = authority.strip_prefix('[') {
        inner.split(']').next().unwrap_or_default()
    } else {
        authority.split(':').next().unwrap_or_default()
    };
    matches!(host, "127.0.0.1" | "localhost" | "::1")
}

/// An API key. Never printed, and never part of a request's URL or body.
#[derive(Clone, PartialEq, Eq)]
pub struct Key(String);

impl Key {
    /// Check a pasted key. Whitespace is trimmed; a key with a space *inside* it is a paste
    /// accident and is refused rather than sent to fail at the server.
    pub fn new(raw: &str) -> Result<Self> {
        let key = raw.trim();
        if key.is_empty() {
            return Err(Error::Provider("The API key is empty".into()));
        }
        if key.chars().count() > MAX_KEY {
            return Err(Error::Provider(format!("The API key is longer than {MAX_KEY} characters")));
        }
        if key.chars().any(|c| c.is_control() || c.is_whitespace()) {
            return Err(Error::Provider("The API key contains a space or control character".into()));
        }
        Ok(Self(key.to_string()))
    }

    /// The value, for the transport's header. The only place a key is readable.
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// Enough to tell two keys apart in a status line, never enough to use one.
    pub fn masked(&self) -> String {
        let count = self.0.chars().count();
        if count <= 8 {
            return "****".to_string();
        }
        let tail: String = self.0.chars().skip(count - 4).collect();
        format!("****{tail}")
    }
}

impl std::fmt::Debug for Key {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Key(****)")
    }
}

/// The JSON object in a model's answer: bare, fenced, or wrapped in a sentence.
///
/// Models are asked for one JSON object and nothing else, and most comply; a few add a fence or
/// a line of prose. Unwrapping those here is cheaper and kinder than failing a poster over a
/// stray backtick. Nothing is guessed: if no object parses, it is an error.
pub fn extract_json(answer: &str) -> Result<Value> {
    let text = answer.trim();
    if text.is_empty() {
        return Err(Error::Provider("The model answered with nothing".into()));
    }
    if let Ok(value) = serde_json::from_str::<Value>(text)
        && value.is_object()
    {
        return Ok(value);
    }
    if let Some(inner) = fenced(text)
        && let Ok(value) = serde_json::from_str::<Value>(inner)
        && value.is_object()
    {
        return Ok(value);
    }
    if let Some(value) = braced(text) {
        return Ok(value);
    }
    Err(Error::Provider("The model's answer held no JSON object".into()))
}

/// The body of the first ``` fence, without its language tag.
fn fenced(text: &str) -> Option<&str> {
    let start = text.find("```")?;
    let after = text.get(start + 3..)?;
    let after = after.strip_prefix("json").or_else(|| after.strip_prefix("JSON")).unwrap_or(after);
    let after = after.trim_start_matches(['\r', '\n', ' ']);
    let end = after.find("```")?;
    Some(after.get(..end)?.trim())
}

/// The first `{` to the last `}`, when that parses as an object.
fn braced(text: &str) -> Option<Value> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    if end <= start {
        return None;
    }
    let slice = text.get(start..=end)?;
    serde_json::from_str::<Value>(slice).ok().filter(Value::is_object)
}

/// Longest transport failure or server message we pass on. It becomes one line in the panel.
pub const MAX_REASON: usize = 200;

/// A failure flattened to one bounded line, so it fits in a panel's status area.
///
/// Control characters (a stack trace, a `\r\n`) become spaces, and a long message is cut: the
/// panel has one line, and a person reads the first sentence of a diagnostic, not the whole of it.
pub fn one_line(text: &str) -> String {
    let flat: String = text.chars().map(|c| if c.is_control() { ' ' } else { c }).take(MAX_REASON).collect();
    if text.chars().count() > MAX_REASON { format!("{}…", flat.trim_end()) } else { flat.trim().to_string() }
}

/// What a refused request says: the server's own message when it sent one.
///
/// Every OpenAI-compatible endpoint answers an error with `{"error": {"message": "…"}}`, and that
/// sentence ("Incorrect API key provided") is worth more than the status alone. A server that
/// sends something else gets its body flattened to one bounded line instead.
pub fn refusal(status: u16, body: &str) -> String {
    let message = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|value| value.pointer("/error/message").and_then(|message| message.as_str()).map(str::to_string))
        .or_else(|| {
            let text = body.trim();
            (!text.is_empty()).then(|| one_line(text))
        });
    match message {
        Some(message) => format!("The model server refused ({status}): {message}"),
        None => format!("The model server refused ({status}) with no explanation"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_bare_host_gets_the_versioned_path_and_a_path_keeps_its_own() {
        let cases = [
            ("https://api.openai.com", "https://api.openai.com/v1/chat/completions"),
            ("https://api.openai.com/", "https://api.openai.com/v1/chat/completions"),
            ("https://api.openai.com/v1", "https://api.openai.com/v1/chat/completions"),
            ("https://api.openai.com/v1/", "https://api.openai.com/v1/chat/completions"),
            ("https://generativelanguage.googleapis.com/v1beta/openai", "https://generativelanguage.googleapis.com/v1beta/openai/chat/completions"),
            ("http://127.0.0.1:11434", "http://127.0.0.1:11434/v1/chat/completions"),
            ("http://localhost:1234/v1", "http://localhost:1234/v1/chat/completions"),
            ("https://openrouter.ai/api/v1", "https://openrouter.ai/api/v1/chat/completions"),
            ("https://example.test/v1/chat/completions", "https://example.test/v1/chat/completions"),
        ];
        for (base, url) in cases {
            let endpoint = Endpoint::new(base, "m").expect(base);
            assert_eq!(endpoint.chat_url(), url, "{base}");
        }
    }

    #[test]
    fn a_base_that_would_leak_the_key_is_refused() {
        for base in [
            "",
            "   ",
            "api.openai.com",
            "ftp://example.test",
            "https://user:secret@example.test/v1",
            "https://example.test/v1?key=sk-1",
            "https://example.test/v1#frag",
            "https://exam ple.test",
            "https://example.test/v1\nx",
        ] {
            assert!(Endpoint::new(base, "m").is_err(), "{base:?} was accepted");
        }
        // Whitespace a paste leaves at either end is trimmed, not refused.
        assert_eq!(Endpoint::new("  https://example.test/v1\n", "m").expect("a trimmed base").base(), "https://example.test/v1");
        // Plain http is for a server on this machine only.
        assert!(Endpoint::new("http://192.168.1.9:11434", "m").is_err());
        assert!(Endpoint::new("http://127.0.0.1:11434", "m").is_ok());
        assert!(Endpoint::new("http://[::1]:8080", "m").is_ok());
        assert!(Endpoint::new("http://localhost:8080/v1", "m").is_ok());
    }

    #[test]
    fn a_model_name_is_required_and_bounded() {
        assert!(Endpoint::new("https://example.test", "").is_err());
        assert!(Endpoint::new("https://example.test", "   ").is_err());
        assert!(Endpoint::new("https://example.test", "a b").is_err());
        let long = "x".repeat(MAX_MODEL + 1);
        assert!(Endpoint::new("https://example.test", &long).is_err());
        // Names with slashes, colons and at signs are ordinary.
        for model in ["gpt-4o-mini", "qwen2.5:7b", "accounts/fireworks/models/llama-v3p1-8b", "meta-llama/Llama-3.1-8B@fp16"] {
            assert!(Endpoint::new("https://example.test", model).is_ok(), "{model}");
        }
    }

    #[test]
    fn the_host_is_what_a_status_line_needs() {
        let endpoint = Endpoint::new("https://api.openai.com/v1", "gpt-4o-mini").expect("an endpoint");
        assert_eq!(endpoint.host(), "api.openai.com");
        assert_eq!(endpoint.model(), "gpt-4o-mini");
        let local = Endpoint::new("http://127.0.0.1:11434", "llama3").expect("an endpoint");
        assert_eq!(local.host(), "127.0.0.1:11434");
    }

    #[test]
    fn a_key_never_prints_and_never_keeps_a_paste_accident() {
        let key = Key::new("  sk-abcdefghijklmnop  ").expect("a key");
        assert_eq!(key.expose(), "sk-abcdefghijklmnop");
        assert_eq!(key.masked(), "****mnop");
        assert!(!format!("{key:?}").contains("sk-"));
        assert_eq!(Key::new("short").expect("a key").masked(), "****");
        let long = "x".repeat(MAX_KEY + 1);
        for bad in ["", "   ", "sk-abc def", "sk-abc\ndef", long.as_str()] {
            assert!(Key::new(bad).is_err(), "{bad:?} was accepted");
        }
    }

    #[test]
    fn an_answer_is_unwrapped_from_a_fence_or_a_sentence() {
        let wanted = json!({"title": "Autumn Print Fair"});
        let cases = [
            wanted.to_string(),
            format!("```json\n{wanted}\n```"),
            format!("```\n{wanted}\n```"),
            format!("Here is the brief:\n{wanted}\nEnjoy!"),
            format!("  \n{wanted}  \n"),
        ];
        for case in cases {
            assert_eq!(extract_json(&case).expect(&case), wanted, "{case:?}");
        }
        for bad in ["", "   ", "I cannot help with that.", "[1,2,3]", "{not json}"] {
            assert!(extract_json(bad).is_err(), "{bad:?} was accepted");
        }
    }

    #[test]
    fn a_fake_records_the_request_and_answers_from_its_script() {
        let fake = Fake::answering("{}");
        let copy = fake.clone();
        assert_eq!(fake.post_json("https://example.test/x", &[("Authorization", "Bearer k")], "{\"a\":1}").expect("an answer"), "{}");
        let seen = copy.seen();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].url, "https://example.test/x");
        assert_eq!(seen[0].headers, vec![("Authorization".to_string(), "Bearer k".to_string())]);
        assert_eq!(seen[0].body, "{\"a\":1}");
        assert!(Fake::failing("no route to host").post_json("https://example.test", &[], "{}").is_err());
    }

    #[test]
    fn a_refused_request_carries_the_servers_own_message() {
        let body = json!({"error": {"message": "Incorrect API key provided"}}).to_string();
        assert_eq!(refusal(401, &body), "The model server refused (401): Incorrect API key provided");
        assert_eq!(refusal(502, "<html>bad gateway</html>"), "The model server refused (502): <html>bad gateway</html>");
        assert_eq!(refusal(500, "   "), "The model server refused (500) with no explanation");
        // A message is flattened and bounded, never a page of stack trace.
        let long = "x".repeat(10_000);
        let message = refusal(500, &long);
        assert!(message.chars().count() < 300, "{}", message.chars().count());
        // And a control character never becomes a second line in the panel.
        assert!(!one_line("a\nb\rc").contains('\n'));
        assert_eq!(one_line("  padded  "), "padded");
        assert!(one_line(&long).chars().count() <= MAX_REASON + 1);
    }
}
