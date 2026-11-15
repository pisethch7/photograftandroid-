//! The model call: a trait the app implements, and the prompts it sends.
//!
//! This crate stays off the network on purpose. A [`Planner`] turns a [`Request`] into a [`Brief`];
//! the implementation lives in the app, because the two targets need different transports (a Rust
//! HTTP client on desktop, Android's own stack through the Java bridge on a tablet). Tests use
//! [`Scripted`], so a whole poster — prompt, answer, design, document — is exercised with no key
//! and no connection.
//!
//! The prompts are here rather than in the app for one reason: [`brief::schema`] is what the brief
//! actually accepts, so the instruction the model reads and the validation it is held to cannot
//! drift apart.

use serde_json::{Value, json};

use crate::brief::{self, Brief, MAX_STYLE, MAX_TEXT, MAX_TITLE};
use crate::error::{Error, Result};

/// Longest prompt a person (or a calling agent) may send.
pub const MAX_PROMPT: usize = 4000;

/// What to ask for. The user's words and the choices already made in the UI.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Request {
    /// What the poster is for, in the user's own words.
    pub prompt: String,
    /// A preset id chosen in the UI, if any.
    pub format: Option<String>,
    /// A template id chosen in the UI, if any.
    pub template: Option<String>,
    /// Mood hints chosen in the UI.
    pub style: Vec<String>,
}

impl Request {
    /// A request from a prompt alone.
    pub fn new(prompt: impl Into<String>) -> Self {
        Self { prompt: prompt.into(), ..Self::default() }
    }

    /// Check the words and the choices before anything is sent to a model.
    pub fn validate(&self) -> Result<()> {
        let prompt = self.prompt.trim();
        if prompt.is_empty() {
            return Err(Error::field("prompt", "is empty"));
        }
        if prompt.chars().count() > MAX_PROMPT {
            return Err(Error::field("prompt", format!("is {} characters; at most {MAX_PROMPT} are sent", prompt.chars().count())));
        }
        if prompt.chars().any(|c| c.is_control() && c != '\n' && c != '\t') {
            return Err(Error::field("prompt", "contains a control character"));
        }
        if let Some(format) = &self.format
            && crate::format::preset(format).is_none()
        {
            return Err(Error::Format(format!("{format:?} is not a preset")));
        }
        if let Some(template) = &self.template
            && crate::template::resolve(template.trim()).is_none()
        {
            return Err(Error::Template(format!("{template:?} is not a template")));
        }
        if self.style.len() > MAX_STYLE {
            return Err(Error::field("style", format!("{} hints; at most {MAX_STYLE} are read", self.style.len())));
        }
        Ok(())
    }
}

/// Something that can turn a request into a brief: a cloud model, a local one, or a test double.
pub trait Planner {
    /// Ask for the brief. Blocks: callers run it off the UI thread.
    fn brief(&self, request: &Request) -> Result<Brief>;
    /// Where the answer came from, for the status bar and the log ("openai/gpt-…", "scripted").
    fn describe(&self) -> String {
        "planner".to_string()
    }
}

/// A test double: answers with a brief it was given, so the whole path can be tested offline.
#[derive(Clone, Debug, PartialEq)]
pub struct Scripted {
    answer: Value,
    label: String,
}

impl Scripted {
    /// Answer every request with this JSON, which is validated like any model's answer.
    pub fn new(answer: Value) -> Self {
        Self { answer, label: "scripted".to_string() }
    }

    /// Answer with a brief that already validates.
    pub fn from_brief(brief: &Brief) -> Result<Self> {
        let answer = serde_json::to_value(brief).map_err(|error| Error::Json(error.to_string()))?;
        Ok(Self::new(answer))
    }

    /// Name the double in logs (a test may want two of them to tell apart).
    pub fn labelled(mut self, label: impl Into<String>) -> Self {
        self.label = label.into();
        self
    }
}

impl Planner for Scripted {
    fn brief(&self, request: &Request) -> Result<Brief> {
        request.validate()?;
        Brief::from_json(&self.answer)
    }

    fn describe(&self) -> String {
        self.label.clone()
    }
}

/// The instruction a model reads: the brief's fields, the bounds, and the rules that keep it
/// away from layout and commands.
pub fn system_prompt() -> String {
    format!(
        "You design posters. You answer with one JSON object and nothing else: no prose, no code \
fences, no comments.\n\n\
The object is a poster brief. PhotoCraft lays it out itself, so:\n\
- never send coordinates, font sizes, pixel sizes, colours per element, or any command;\n\
- write the words a person would want on the poster, not descriptions of them;\n\
- pick a palette that suits the subject: three #rrggbb colours that read well together;\n\
- `title` is the headline and is the only required field;\n\
- keep the title under {MAX_TITLE} characters and each other text under {MAX_TEXT}.\n\n\
{brief}\n",
        brief = brief::schema(),
    )
}

/// The user's turn: what they asked for, plus the choices already made in the interface.
pub fn user_prompt(request: &Request) -> String {
    let mut s = format!("Make a poster: {}", request.prompt.trim());
    if let Some(format) = &request.format {
        s.push_str(&format!("\nPage: {format}"));
    }
    if let Some(template) = &request.template {
        s.push_str(&format!("\nLayout: {template}"));
    }
    if !request.style.is_empty() {
        s.push_str(&format!("\nMood: {}", request.style.join(", ")));
    }
    s
}

/// The answer as the engine's `brief` parameter, ready for `ai.poster.plan` / `ai.poster.build`.
pub fn build_params(brief: &Brief, name: Option<&str>) -> Result<Value> {
    let brief = serde_json::to_value(brief).map_err(|error| Error::Json(error.to_string()))?;
    Ok(match name.filter(|name| !name.trim().is_empty()) {
        Some(name) => json!({ "brief": brief, "name": name }),
        None => json!({ "brief": brief }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample() -> Brief {
        Brief::from_json(&json!({
            "title": "Autumn Print Fair",
            "subtitle": "Twelve studios open late",
            "body": "Prints, posters and new work.",
            "footer": "Sat 14 Nov · 6-9 pm",
            "format": "a4",
        }))
        .expect("valid")
    }

    #[test]
    fn a_scripted_planner_answers_with_its_brief() {
        let brief = sample();
        let planner = Scripted::from_brief(&brief).expect("a scripted planner");
        let answer = planner.brief(&Request::new("autumn print fair")).expect("an answer");
        assert_eq!(answer.title, "Autumn Print Fair");
        assert_eq!(planner.describe(), "scripted");
        assert_eq!(Scripted::from_brief(&brief).expect("again").labelled("fake-1").describe(), "fake-1");
    }

    #[test]
    fn the_planner_still_validates_the_answer() {
        let planner = Scripted::new(json!({ "title": "" }));
        assert!(planner.brief(&Request::new("poster")).is_err());
        let planner = Scripted::new(json!({ "title": "T", "version": 7 }));
        assert!(planner.brief(&Request::new("poster")).is_err());
    }

    #[test]
    fn an_empty_or_absurd_prompt_is_rejected_before_anything_is_sent() {
        let planner = Scripted::from_brief(&sample()).expect("a planner");
        assert!(planner.brief(&Request::new("   ")).is_err());
        assert!(planner.brief(&Request::new("x".repeat(MAX_PROMPT + 1))).is_err());
        assert!(planner.brief(&Request::new("bell \u{7}")).is_err());
        let mut bad = Request::new("poster");
        bad.format = Some("a9".into());
        assert!(planner.brief(&bad).is_err());
        let mut bad = Request::new("poster");
        bad.template = Some("spiral".into());
        assert!(planner.brief(&bad).is_err());
        let mut bad = Request::new("poster");
        bad.style = vec!["warm".into(); MAX_STYLE + 1];
        assert!(planner.brief(&bad).is_err());
    }

    #[test]
    fn the_system_prompt_teaches_the_schema_the_brief_accepts() {
        let prompt = system_prompt();
        for id in crate::format::preset_ids() {
            assert!(prompt.contains(id), "{id} missing from the prompt");
        }
        for id in crate::template::ids() {
            assert!(prompt.contains(id), "{id} missing from the prompt");
        }
        assert!(prompt.contains("\"title\""));
        assert!(prompt.contains("nothing else"), "the model must answer with JSON only");
        assert!(prompt.contains("never send coordinates"), "the model must not place anything");
        assert!(prompt.contains(&MAX_TITLE.to_string()));
    }

    #[test]
    fn the_user_prompt_carries_the_words_and_the_choices() {
        let mut request = Request::new("  a night market for ceramics  ");
        request.format = Some("story".into());
        request.template = Some("hero".into());
        request.style = vec!["warm".into(), "minimal".into()];
        let prompt = user_prompt(&request);
        assert!(prompt.starts_with("Make a poster: a night market for ceramics"));
        assert!(prompt.contains("Page: story"));
        assert!(prompt.contains("Layout: hero"));
        assert!(prompt.contains("Mood: warm, minimal"));
        let plain = user_prompt(&Request::new("just this"));
        assert_eq!(plain, "Make a poster: just this");
    }

    #[test]
    fn build_params_are_what_the_command_takes() {
        let brief = sample();
        let params = build_params(&brief, None).expect("params");
        assert!(params.get("brief").is_some_and(Value::is_object));
        assert!(params.get("name").is_none());
        let named = build_params(&brief, Some("  Autumn  ")).expect("params");
        assert_eq!(named["name"], "  Autumn  ");
        assert!(build_params(&brief, Some("   ")).expect("params").get("name").is_none());
        // And the engine accepts them: the same validation the command runs.
        assert!(Brief::from_json(&params["brief"]).is_ok());
    }
}
