//! The OpenAI-compatible chat-completions client: the one provider this build ships.
//!
//! "OpenAI-compatible" is a de-facto standard rather than a vendor: OpenAI, Gemini's compat
//! endpoint, Azure OpenAI, OpenRouter, Groq, vLLM, LM Studio and Ollama all answer
//! `POST {base}/chat/completions` with `{"choices":[{"message":{"content": "…"}}]}`. One client
//! therefore covers a cloud key and a model on the local network, which is why the setting is a
//! base URL and a model name rather than a vendor name.
//!
//! The request is deliberately plain: the model, the two messages, and `stream: false`. No
//! `temperature` (reasoning models refuse it) and no `response_format` (some compatible servers
//! answer 400 for it). The system prompt asks for one JSON object, and
//! [`http::extract_json`](crate::http::extract_json) unwraps a fence or a stray sentence, so the
//! widest range of servers works without a per-vendor branch.

use serde_json::{Value, json};

use crate::brief::Brief;
use crate::error::{Error, Result};
use crate::http::{Endpoint, Key, Transport, extract_json};
use crate::provider::{Planner, Request, system_prompt, user_prompt};

/// Longest answer we will parse. A brief is about a kilobyte; a server that sends megabytes is
/// either broken or not a model endpoint, and either way must not be copied into memory whole.
pub const MAX_ANSWER: usize = 256 * 1024;

/// A planner that asks an OpenAI-compatible endpoint.
pub struct OpenAi<T> {
    endpoint: Endpoint,
    key: Option<Key>,
    transport: T,
}

impl<T: Transport> OpenAi<T> {
    pub fn new(endpoint: Endpoint, key: Option<Key>, transport: T) -> Self {
        Self { endpoint, key, transport }
    }

    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    /// The chat-completions request body, before the transport sees it.
    pub fn request_body(&self, request: &Request) -> Value {
        json!({
            "model": self.endpoint.model(),
            "messages": [
                { "role": "system", "content": system_prompt() },
                { "role": "user", "content": user_prompt(request) },
            ],
            "stream": false,
        })
    }

    /// Read the brief out of a server's answer.
    ///
    /// The answer is untrusted: it may be an error document, a model refusal, a fence, or a
    /// brief with a field out of range. Every one of those is an [`Error`], and the brief it does
    /// produce is validated exactly as a brief typed into `ai.poster.build` is.
    pub fn parse(&self, body: &str) -> Result<Brief> {
        if body.len() > MAX_ANSWER {
            return Err(Error::Provider(format!("The server's answer is {} bytes; at most {MAX_ANSWER} are read", body.len())));
        }
        let value: Value = serde_json::from_str(body).map_err(|error| Error::Provider(format!("The server's answer was not JSON: {error}")))?;
        if let Some(message) = value.pointer("/error/message").and_then(Value::as_str) {
            return Err(Error::Provider(format!("The server refused: {}", clip(message))));
        }
        let content = message_content(&value).ok_or_else(|| Error::Provider("The server's answer held no message".into()))?;
        Brief::from_json(&extract_json(content)?)
    }
}

impl<T: Transport> Planner for OpenAi<T> {
    fn brief(&self, request: &Request) -> Result<Brief> {
        request.validate()?;
        let body = self.request_body(request).to_string();
        // The key lives only in this header: not in the URL, not in the body, and not in a log.
        let authorization = self.key.as_ref().map(|key| format!("Bearer {}", key.expose()));
        let mut headers: Vec<(&str, &str)> = vec![("Content-Type", "application/json"), ("Accept", "application/json")];
        if let Some(value) = &authorization {
            headers.push(("Authorization", value));
        }
        let answer = self.transport.post_json(&self.endpoint.chat_url(), &headers, &body)?;
        self.parse(&answer)
    }

    fn describe(&self) -> String {
        format!("{}/{}", self.endpoint.host(), self.endpoint.model())
    }
}

/// `choices[0].message.content`, which some servers send as text parts instead of a string, and
/// a legacy completion's `choices[0].text`.
fn message_content(value: &Value) -> Option<&str> {
    let choice = value.get("choices")?.get(0)?;
    if let Some(text) = choice.pointer("/message/content").and_then(Value::as_str) {
        return Some(text);
    }
    if let Some(text) = choice.get("text").and_then(Value::as_str) {
        return Some(text);
    }
    choice.pointer("/message/content")?.as_array()?.iter().find_map(|part| part.get("text").and_then(Value::as_str))
}

/// A server's message, flattened to one bounded line so it fits in a panel's status line.
fn clip(text: &str) -> String {
    let flat: String = text.chars().map(|c| if c.is_control() { ' ' } else { c }).take(300).collect();
    if text.chars().count() > 300 { format!("{}…", flat.trim_end()) } else { flat.trim().to_string() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::Fake;
    use serde_json::json;

    fn request() -> Request {
        Request::new("a night market for ceramics")
    }

    fn answer(content: &str) -> String {
        json!({ "choices": [{ "message": { "role": "assistant", "content": content } }] }).to_string()
    }

    fn planner(fake: Fake, key: Option<Key>) -> OpenAi<Fake> {
        OpenAi::new(Endpoint::new("https://api.example.test/v1", "gpt-4o-mini").expect("an endpoint"), key, fake)
    }

    #[test]
    fn the_request_is_the_one_every_compatible_server_reads() {
        let fake = Fake::answering(answer(r#"{"title": "Night Market"}"#));
        let client = planner(fake.clone(), Some(Key::new("sk-test-key").expect("a key")));
        let brief = client.brief(&request()).expect("a brief");
        assert_eq!(brief.title, "Night Market");

        let seen = fake.seen();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].url, "https://api.example.test/v1/chat/completions");
        let body: Value = serde_json::from_str(&seen[0].body).expect("JSON");
        assert_eq!(body["model"], "gpt-4o-mini");
        assert_eq!(body["stream"], false);
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["messages"][1]["role"], "user");
        assert!(body["messages"][1]["content"].as_str().unwrap_or_default().contains("a night market for ceramics"));
        assert!(body.get("temperature").is_none(), "a reasoning model would refuse it");
        assert!(body.get("response_format").is_none(), "some compatible servers answer 400 for it");
        // The key travels in one header, and nowhere else.
        let auth: Vec<&(String, String)> = seen[0].headers.iter().filter(|(name, _)| name == "Authorization").collect();
        assert_eq!(auth.len(), 1);
        assert_eq!(auth.first().map(|(_, value)| value.as_str()), Some("Bearer sk-test-key"));
        assert!(!seen[0].body.contains("sk-test-key"), "the key must not be in the body");
        assert!(!seen[0].url.contains("sk-test-key"), "the key must not be in the URL");
    }

    #[test]
    fn a_local_server_with_no_key_sends_no_authorization() {
        let fake = Fake::answering(answer(r#"{"title": "Local"}"#));
        let client = OpenAi::new(Endpoint::new("http://127.0.0.1:11434", "llama3").expect("an endpoint"), None, fake.clone());
        client.brief(&request()).expect("a brief");
        let seen = fake.seen();
        assert!(seen[0].headers.iter().all(|(name, _)| name != "Authorization"), "{:?}", seen[0].headers);
        assert_eq!(seen[0].url, "http://127.0.0.1:11434/v1/chat/completions");
    }

    #[test]
    fn a_fenced_or_chatty_answer_still_becomes_a_brief() {
        for content in [
            r#"{"title": "Fenced", "format": "a4"}"#.to_string(),
            format!("```json\n{}\n```", r#"{"title": "Fenced", "format": "a4"}"#),
            format!("Sure! Here is the brief:\n{}\nLet me know.", r#"{"title": "Fenced", "format": "a4"}"#),
        ] {
            let client = planner(Fake::answering(answer(&content)), None);
            assert_eq!(client.brief(&request()).expect(&content).title, "Fenced");
        }
    }

    #[test]
    fn an_answer_with_text_parts_is_read_too() {
        let body = json!({ "choices": [{ "message": { "content": [{ "type": "text", "text": r#"{"title": "Parts"}"# }] } }] }).to_string();
        assert_eq!(planner(Fake::answering(body), None).brief(&request()).expect("a brief").title, "Parts");
        let legacy = json!({ "choices": [{ "text": r#"{"title": "Legacy"}"# }] }).to_string();
        assert_eq!(planner(Fake::answering(legacy), None).brief(&request()).expect("a brief").title, "Legacy");
    }

    #[test]
    fn every_kind_of_bad_answer_is_a_sentence_not_a_panic() {
        let cases = [
            (String::new(), "not JSON"),
            ("<html>502</html>".to_string(), "not JSON"),
            (json!({"error": {"message": "Incorrect API key provided"}}).to_string(), "Incorrect API key provided"),
            (json!({"choices": []}).to_string(), "no message"),
            (json!({"choices": [{}]}).to_string(), "no message"),
            (answer("I cannot help with that."), "no JSON object"),
            (answer(r#"{"title": ""}"#), "title"),
            (answer(r#"{"title": "T", "format": "nope"}"#), "format"),
            (answer(r#"{"title": "T", "version": 99}"#), "version"),
            ("x".repeat(MAX_ANSWER + 1), "at most"),
        ];
        for (body, expected) in cases {
            let error = planner(Fake::answering(body.clone()), None).brief(&request()).expect_err(&body);
            let message = error.to_string();
            assert!(message.contains(expected), "{body:.80} → {message}");
            assert!(message.len() < 400, "a panel line, not a page: {message}");
        }
    }

    #[test]
    fn a_refused_connection_reaches_the_person_as_a_sentence() {
        let client = planner(Fake::failing("no route to host"), None);
        let error = client.brief(&request()).expect_err("a failure");
        assert_eq!(error.to_string(), "no route to host");
    }

    #[test]
    fn an_empty_prompt_is_refused_before_the_network_is_touched() {
        let fake = Fake::answering(answer(r#"{"title": "Never sent"}"#));
        let client = planner(fake.clone(), None);
        assert!(client.brief(&Request::new("   ")).is_err());
        assert!(fake.seen().is_empty(), "nothing was sent");
    }

    #[test]
    fn the_description_names_the_host_and_model_never_the_key() {
        let client = planner(Fake::answering("{}"), Some(Key::new("sk-secret-value").expect("a key")));
        let described = client.describe();
        assert_eq!(described, "api.example.test/gpt-4o-mini");
        assert!(!described.contains("sk-secret"));
    }
}
