//! `ai.*` commands: an AI poster brief becomes an editable document.
//!
//! The AI never draws and never writes commands. It writes a **brief** — title, copy, palette,
//! mood, page — and this command validates it ([`photocraft_ai::Brief`]), composes it into one of
//! five templates, and runs the ordinary `file.new` / `layer.newFillLayer.*` / `shape.create` /
//! `type.create` commands that build it. What lands on the canvas is therefore a normal document:
//! every word is a type layer that can be retyped, every panel is a shape layer that can be
//! reshaped, and a plug-in can filter any of them.
//!
//! Atomicity: `ai.poster.build` opens the document, builds every layer, and only then clears the
//! history so the poster arrives as one finished document (like File › Open, in the state the AI
//! returned it, not as twelve undo steps). If a step fails, the half-built document is closed, so
//! a bad brief leaves the open documents exactly as they were.
//!
//! Two halves, joined by a channel. The model call needs a socket, so the app injects it
//! ([`PlanFn`], the same shape as `updates_cmds::CheckFn`); the engine owns the answer, so the
//! CLI, the control channel, MCP and the panel all read one state. `ai.poster.create` starts the
//! call and returns at once, `ai.poster.status` polls it, and its answer carries the parameters
//! `ai.poster.build` takes — so an agent drives prompt → document in three calls.
//!
//! Nothing here talks to a network or holds a secret: command parameters are journaled and
//! replayed, so an API key must never be one (a `key` parameter is refused by name). The provider
//! settings live in the app, outside the engine.

use std::sync::mpsc::{Receiver, TryRecvError};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::commands::{CommandSpec, always};
use crate::{EngineError, Result, Session};
use photocraft_ai::{Brief, Created, Request};

/// Composing and checking a brief without touching a document.
pub const PLAN: &str = "ai.poster.plan";
/// Building the document.
pub const BUILD: &str = "ai.poster.build";
/// The presets, templates and brief schema the UI offers.
pub const PRESETS_CMD: &str = "ai.poster.presets";
/// Asking a model for a brief, in the background.
pub const CREATE: &str = "ai.poster.create";
/// Reading what the background request came to.
pub const STATUS: &str = "ai.poster.status";
/// Giving up on a request that is still running.
pub const CANCEL: &str = "ai.poster.cancel";

/// What the app's planner thread sends back: a validated brief, or a sentence a person can act on.
pub type PlanResult = std::result::Result<Brief, String>;

/// Starts a model call and returns at once, without blocking the caller.
///
/// The implementation lives in the app: a Rust HTTP client on desktop, Android's own stack on a
/// tablet, and a scripted double in tests. `Err` means the request could not even start (no
/// settings, a bad URL) and its message goes straight to the panel.
pub type PlanFn = Box<dyn FnMut(&Request) -> std::result::Result<Receiver<PlanResult>, String> + Send>;

/// Where an AI poster request has got to.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum PosterStatus {
    #[default]
    Idle,
    /// The request is out; `ai.poster.status` polls it.
    Planning,
    /// The answer: the same fields `ai.poster.plan` returns, plus `brief` and `params`.
    Ready {
        answer: Value,
    },
    Failed {
        message: String,
    },
}

/// The background request, and what it came to.
#[derive(Default)]
pub struct Posters {
    /// The app's transport. `None` in a frontend that has no model call (the web build today).
    pub plan: Option<PlanFn>,
    pub status: PosterStatus,
    pending: Option<Receiver<PlanResult>>,
}

impl Posters {
    /// Consume a finished request; a disconnected worker becomes an actionable error.
    ///
    /// Returns whether anything changed, so the panel knows to repaint.
    pub fn poll(&mut self) -> bool {
        let Some(rx) = &self.pending else { return false };
        let result = match rx.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return false,
            Err(TryRecvError::Disconnected) => Err("The AI request stopped before it answered".into()),
        };
        self.pending = None;
        self.status = match result {
            Ok(brief) => match answer_of(&brief) {
                Ok(answer) => PosterStatus::Ready { answer },
                // The brief validated in the app but does not compose: a bug, reported rather
                // than panicked.
                Err(error) => PosterStatus::Failed { message: error },
            },
            Err(message) => PosterStatus::Failed { message },
        };
        true
    }

    /// Whether a request is out.
    pub fn busy(&self) -> bool {
        self.pending.is_some()
    }
}

fn bad(cmd: &str, message: impl Into<String>) -> EngineError {
    EngineError::BadParams { cmd: cmd.into(), msg: message.into() }
}

/// Read the `brief` parameter and validate it; the document name defaults to the brief's title.
fn parse(cmd: &str, params: &Value) -> Result<(Brief, String)> {
    let brief_json = params.get("brief").ok_or_else(|| bad(cmd, "missing `brief`: the poster brief object"))?;
    if !brief_json.is_object() {
        return Err(bad(cmd, "`brief` must be an object"));
    }
    let brief = Brief::from_json(brief_json).map_err(|error| bad(cmd, error.to_string()))?;
    let name =
        params.get("name").and_then(Value::as_str).map(str::trim).filter(|name| !name.is_empty()).map(str::to_string).unwrap_or_else(|| brief.title.clone());
    Ok((brief, name))
}

/// A brief composed into the fields the panel previews and `ai.poster.build` takes.
///
/// `ai.poster.plan` and a finished `ai.poster.create` return the same shape, so the preview does
/// not care which one produced it.
fn answer_of(brief: &Brief) -> std::result::Result<Value, String> {
    let (design, steps) = photocraft_ai::poster(brief, &brief.title).map_err(|error| error.to_string())?;
    let brief_json = serde_json::to_value(brief).map_err(|error| error.to_string())?;
    let params = photocraft_ai::provider::build_params(brief, None).map_err(|error| error.to_string())?;
    Ok(json!({
        "brief": brief_json,
        "params": params,
        "template": design.template,
        "size": design.format.describe(),
        "paper": design.paper.hex(),
        "width": design.format.width,
        "height": design.format.height,
        // The preview converts type points to pixels at this resolution, exactly as `type.create`
        // will; a 300 dpi A4 headline is 59.5 pt, not 248 px.
        "dpi": design.format.dpi,
        "layers": design.layer_count(),
        "names": design.elements.iter().map(|element| element.name()).collect::<Vec<_>>(),
        "steps": steps.iter().map(|step| step.to_json()).collect::<Vec<_>>(),
        "hints": design.style,
    }))
}

/// `ai.poster.plan`: compose the brief into a design and its steps, changing nothing.
fn plan(_: &mut Session, params: &Value) -> Result<Value> {
    let (brief, _) = parse(PLAN, params)?;
    answer_of(&brief).map_err(|error| bad(PLAN, error))
}

/// `ai.poster.build`: the whole thing, as a new document.
fn build(s: &mut Session, params: &Value) -> Result<Value> {
    let (brief, name) = parse(BUILD, params)?;
    let (design, steps) = photocraft_ai::poster(&brief, &name).map_err(|error| bad(BUILD, error.to_string()))?;
    let open_before = s.documents().len();
    let mut created = Created::default();
    let mut opened: Option<usize> = None;
    for (at, step) in steps.iter().enumerate() {
        let resolved = created.resolve(&step.params).map_err(|error| bad(BUILD, error))?;
        match s.execute(step.command, resolved) {
            Ok(result) => {
                if at == 0 {
                    opened = Some(
                        result
                            .get("document")
                            .and_then(Value::as_u64)
                            .map(|index| index as usize)
                            .or_else(|| s.active_index())
                            .ok_or_else(|| bad(BUILD, "the canvas step opened no document"))?,
                    );
                }
                created.push(&result);
            }
            Err(error) => {
                // Either the whole poster arrives or nothing does: take the canvas back out.
                let stray = opened.or_else(|| (s.documents().len() > open_before).then(|| s.active_index()).flatten());
                if let Some(index) = stray {
                    let _ = s.close(index);
                }
                return Err(error);
            }
        }
    }
    let Some(index) = opened else {
        return Err(bad(BUILD, "the plan had no canvas step"));
    };
    // A finished document starts with an empty history, as File › Open's does: the build's own
    // steps are not something to undo into a half-made poster.
    if let Some(state) = s.active_mut() {
        state.history.clear();
        state.bump_revision(true);
    }
    Ok(json!({
        "document": index,
        "name": name,
        "template": design.template,
        "layers": design.layer_count(),
        "steps": steps.len(),
    }))
}

/// The presets, templates and the brief schema, for the panel's menus and the model's prompt.
fn presets(_: &mut Session, _: &Value) -> Result<Value> {
    Ok(json!({
        "presets": photocraft_ai::PRESETS.iter().map(|preset| json!({
            "id": preset.id,
            "label": preset.label,
            "size": preset.format.describe(),
        })).collect::<Vec<_>>(),
        "templates": photocraft_ai::template::ids(),
        "brief": photocraft_ai::brief::schema(),
    }))
}

/// A parameter name that would journal a secret. Refused by name, with the reason.
const SECRET_PARAMS: &[&str] = &["key", "apiKey", "api_key", "token", "authorization", "secret"];

/// Read the prompt and the choices for `ai.poster.create`, and refuse a secret by name.
fn request_from(cmd: &str, params: &Value) -> Result<Request> {
    let Some(fields) = params.as_object() else {
        return Err(bad(cmd, "expected an object with a `prompt`"));
    };
    for name in SECRET_PARAMS {
        if fields.contains_key(*name) {
            return Err(bad(
                cmd,
                format!("`{name}` is never a command parameter: parameters are journaled and replayed. Put the key in the panel's provider settings"),
            ));
        }
    }
    let prompt = fields.get("prompt").and_then(Value::as_str).ok_or_else(|| bad(cmd, "missing `prompt`: what the poster is for"))?;
    let format = fields.get("format").and_then(Value::as_str).map(str::trim).filter(|value| !value.is_empty()).map(str::to_string);
    let template = fields.get("template").and_then(Value::as_str).map(str::trim).filter(|value| !value.is_empty()).map(str::to_string);
    let style = match fields.get("style") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(items)) => items.iter().filter_map(Value::as_str).map(str::to_string).collect(),
        Some(_) => return Err(bad(cmd, "`style` must be an array of mood words")),
    };
    let request = Request { prompt: prompt.to_string(), format, template, style };
    request.validate().map_err(|error| bad(cmd, error.to_string()))?;
    Ok(request)
}

fn empty_params(p: &Value) -> Result<()> {
    if p.as_object().is_some_and(|p| p.is_empty()) { Ok(()) } else { Err(EngineError::Other("this AI command takes an empty object".into())) }
}

fn enabled(s: &Session) -> std::result::Result<(), String> {
    s.ai.plan.as_ref().map(|_| ()).ok_or_else(|| "AI planning is unavailable in this frontend".into())
}

/// `ai.poster.create`: start the model call, and return at once.
fn create(s: &mut Session, params: &Value) -> Result<Value> {
    let request = request_from(CREATE, params)?;
    // A repeated request (an automation loop, a double click) must not spawn a second worker:
    // `ai.poster.cancel` is how a person gives up on the first.
    if s.ai.pending.is_none() {
        let start = s.ai.plan.as_mut().ok_or_else(|| bad(CREATE, "AI planning is unavailable in this frontend"))?;
        match start(&request) {
            Ok(rx) => {
                s.ai.pending = Some(rx);
                s.ai.status = PosterStatus::Planning;
            }
            Err(message) => {
                s.ai.status = PosterStatus::Failed { message: message.clone() };
                return Err(bad(CREATE, message));
            }
        }
    }
    Ok(json!(s.ai.status))
}

/// `ai.poster.status`: poll the request and report where it is.
fn status(s: &mut Session, params: &Value) -> Result<Value> {
    empty_params(params)?;
    s.ai.poll();
    Ok(json!({"supported": s.ai.plan.is_some(), "busy": s.ai.busy(), "result": s.ai.status}))
}

/// `ai.poster.cancel`: stop waiting for a request that is still out.
///
/// The worker itself cannot be interrupted mid-request; dropping its channel means its answer is
/// discarded when it arrives, and the next `create` starts cleanly.
fn cancel(s: &mut Session, params: &Value) -> Result<Value> {
    empty_params(params)?;
    s.ai.pending = None;
    s.ai.status = PosterStatus::Idle;
    Ok(json!({"status": "idle"}))
}

/// The `ai.*` command specs.
pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: PRESETS_CMD,
            label: "Poster Presets",
            menu: &[],
            shortcut: None,
            params: "{} → {presets:[{id,label,size}],templates:[id],brief:schema text}",
            enabled: always,
            run: presets,
            journal: false,
        },
        CommandSpec {
            id: PLAN,
            label: "Plan Poster",
            menu: &[],
            shortcut: None,
            params: r##"{"brief":{"title":text,"subtitle":text?,"body":text?,"footer":text?,"format":preset|custom,"size":[w,h]?,"dpi":n?,"template":id|"auto"?,"palette":{"background":"#rrggbb","ink":"#rrggbb","accent":"#rrggbb"}?,"style":[text]?},"name":text?} → {brief,params,template,size,paper,width,height,layers,names,steps,hints}"##,
            enabled: always,
            run: plan,
            journal: false,
        },
        CommandSpec {
            id: BUILD,
            label: "Create Poster from Brief",
            menu: &[],
            shortcut: None,
            params: r##"{"brief":{… as ai.poster.plan},"name":text?} → {document,name,template,layers,steps} (one new document, empty history)"##,
            enabled: always,
            run: build,
            journal: false,
        },
        CommandSpec {
            id: CREATE,
            label: "Create Poster from a Prompt",
            menu: &[],
            shortcut: None,
            params: r##"{"prompt":text,"format":preset?,"template":id?,"style":[text]?} → {status:"planning"} (the call runs in the background; poll ai.poster.status)"##,
            enabled,
            run: create,
            journal: false,
        },
        CommandSpec {
            id: STATUS,
            label: "AI Poster Status",
            menu: &[],
            shortcut: None,
            params: r##"{} → {supported,busy,result:{status:"idle"|"planning"|"ready"|"failed",answer?,message?}} (a ready answer's `params` go straight to ai.poster.build)"##,
            enabled: always,
            run: status,
            journal: false,
        },
        CommandSpec {
            id: CANCEL,
            label: "Cancel AI Poster",
            menu: &[],
            shortcut: None,
            params: "{} → {status:\"idle\"}",
            enabled: always,
            run: cancel,
            journal: false,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn brief() -> Value {
        json!({
            "title": "Autumn Print Fair",
            "subtitle": "Twelve studios open late",
            "body": "Prints, posters and new work, with the studios open until nine.",
            "footer": "Sat 14 Nov · 6-9 pm · Mill Lane",
            "format": "a4",
        })
    }

    fn session() -> Session {
        Session::new()
    }

    /// A planner that answers at once, with no thread: mpsc is unbounded, so a value sent before
    /// the receiver is returned is already waiting when the engine polls.
    fn scripted(answer: std::result::Result<Value, String>) -> PlanFn {
        Box::new(move |request: &Request| {
            request.validate().map_err(|error| error.to_string())?;
            let (tx, rx) = std::sync::mpsc::channel();
            let sent = match &answer {
                Ok(value) => Brief::from_json(value).map_err(|error| error.to_string()),
                Err(message) => Err(message.clone()),
            };
            tx.send(sent).map_err(|error| error.to_string())?;
            Ok(rx)
        })
    }

    fn with_planner(plan: PlanFn) -> Session {
        let mut s = session();
        s.ai.plan = Some(plan);
        s
    }

    #[test]
    fn a_prompt_becomes_a_brief_and_then_a_document() {
        let mut s = with_planner(scripted(Ok(brief())));
        let started = create(&mut s, &json!({"prompt": "an autumn print fair", "format": "a4"})).expect("started");
        assert_eq!(started["status"], "planning");
        assert!(s.documents().is_empty(), "nothing is built until asked");

        let polled = status(&mut s, &json!({})).expect("polled");
        assert_eq!(polled["supported"], true);
        assert_eq!(polled["result"]["status"], "ready");
        let answer = &polled["result"]["answer"];
        assert_eq!(answer["template"], "stack");
        assert_eq!(answer["width"], 2480);
        assert_eq!(answer["height"], 3508);
        assert!(answer["layers"].as_u64().unwrap_or(0) >= 6);
        assert!(answer["steps"].as_array().is_some_and(|steps| steps.len() >= 6));

        // The answer carries what the build takes, so an agent needs no other call.
        let built = s.execute(BUILD, answer["params"].clone()).expect("built");
        assert_eq!(s.documents().len(), 1);
        assert_eq!(built["layers"], answer["layers"]);
        assert_eq!(s.active().map(|st| st.doc.name.as_str()), Some("Autumn Print Fair"));
    }

    #[test]
    fn the_plan_and_a_finished_request_describe_the_same_poster() {
        let mut s = with_planner(scripted(Ok(brief())));
        let planned = plan(&mut s, &json!({"brief": brief()})).expect("planned");
        create(&mut s, &json!({"prompt": "autumn"})).expect("started");
        let polled = status(&mut s, &json!({})).expect("polled");
        assert_eq!(planned, polled["result"]["answer"], "the preview does not change shape when the model answers");
    }

    #[test]
    fn a_request_that_cannot_start_is_an_error_and_a_failure_that_arrives_is_a_status() {
        let mut s = with_planner(Box::new(|_: &Request| Err("No model is set: name one in the provider settings".into())));
        let error = create(&mut s, &json!({"prompt": "a poster"})).expect_err("refused");
        assert!(error.to_string().contains("No model is set"), "{error}");
        assert_eq!(s.ai.status, PosterStatus::Failed { message: "No model is set: name one in the provider settings".into() });

        // A failure from the network lands in the status, and leaves documents alone.
        let mut s = with_planner(scripted(Err("The server refused: Incorrect API key provided".into())));
        create(&mut s, &json!({"prompt": "a poster"})).expect("started");
        let polled = status(&mut s, &json!({})).expect("polled");
        assert_eq!(polled["result"]["status"], "failed");
        assert!(polled["result"]["message"].as_str().is_some_and(|m| m.contains("Incorrect API key")));
        assert!(s.documents().is_empty());
    }

    #[test]
    fn a_repeated_request_does_not_start_a_second_worker() {
        let starts = Arc::new(AtomicUsize::new(0));
        let counted = starts.clone();
        // Every sender is kept alive, so the request stays in flight: dropping it would disconnect
        // the channel and `status` would (correctly) report a dead worker.
        let held: Arc<std::sync::Mutex<Vec<std::sync::mpsc::Sender<PlanResult>>>> = Arc::default();
        let keep = held.clone();
        let mut s = with_planner(Box::new(move |_: &Request| {
            counted.fetch_add(1, Ordering::SeqCst);
            let (tx, rx) = std::sync::mpsc::channel();
            keep.lock().unwrap().push(tx);
            Ok(rx)
        }));
        for _ in 0..5 {
            assert_eq!(create(&mut s, &json!({"prompt": "a poster"})).expect("started")["status"], "planning");
        }
        assert_eq!(starts.load(Ordering::SeqCst), 1, "one worker, five calls");
        assert!(status(&mut s, &json!({})).expect("polled")["busy"].as_bool().unwrap_or(false));
        // Giving up returns to idle, and the next request starts a fresh worker.
        cancel(&mut s, &json!({})).expect("cancelled");
        assert_eq!(s.ai.status, PosterStatus::Idle);
        create(&mut s, &json!({"prompt": "another"})).expect("started");
        assert_eq!(starts.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn a_worker_that_dies_before_answering_is_an_error_not_a_hang() {
        let mut s = with_planner(Box::new(|_: &Request| {
            let (_tx, rx) = std::sync::mpsc::channel();
            Ok(rx)
        }));
        create(&mut s, &json!({"prompt": "a poster"})).expect("started");
        let polled = status(&mut s, &json!({})).expect("polled");
        assert_eq!(polled["result"]["status"], "failed");
        assert!(polled["result"]["message"].as_str().is_some_and(|m| m.contains("stopped")));
    }

    #[test]
    fn without_a_planner_the_command_says_so_and_status_still_answers() {
        let mut s = session();
        assert!(s.execute(CREATE, json!({"prompt": "a poster"})).is_err());
        let polled = s.execute(STATUS, json!({})).expect("status answers");
        assert_eq!(polled["supported"], false);
        assert_eq!(polled["result"]["status"], "idle");
    }

    #[test]
    fn a_key_is_refused_by_name() {
        let mut s = with_planner(scripted(Ok(brief())));
        for params in [
            json!({"prompt": "a poster", "key": "sk-live-123"}),
            json!({"prompt": "a poster", "apiKey": "sk-live-123"}),
            json!({"prompt": "a poster", "api_key": "sk-live-123"}),
            json!({"prompt": "a poster", "authorization": "Bearer sk-live-123"}),
        ] {
            let error = create(&mut s, &params).expect_err("a key is not a parameter").to_string();
            assert!(error.contains("never a command parameter"), "{error}");
            assert!(!error.contains("sk-live-123"), "the error must not echo the key: {error}");
        }
        assert!(s.journal.is_empty());
    }

    #[test]
    fn hostile_parameters_are_errors_not_crashes() {
        let mut s = with_planner(scripted(Ok(brief())));
        for params in [
            json!(null),
            json!(0),
            json!("prompt"),
            json!([]),
            json!({}),
            json!({"prompt": ""}),
            json!({"prompt": "   "}),
            json!({"prompt": 42}),
            json!({"prompt": "x".repeat(photocraft_ai::provider::MAX_PROMPT + 1)}),
            json!({"prompt": "a poster", "format": "a9"}),
            json!({"prompt": "a poster", "template": "spiral"}),
            json!({"prompt": "a poster", "style": "warm"}),
            json!({"prompt": "a poster", "style": vec!["warm"; 40]}),
        ] {
            assert!(create(&mut s, &params).is_err(), "{params}");
        }
        for params in [json!(null), json!([]), json!({"x": 1}), json!("")] {
            assert!(status(&mut s, &params).is_err(), "{params}");
            assert!(cancel(&mut s, &params).is_err(), "{params}");
        }
        assert!(s.documents().is_empty());
    }

    #[test]
    fn a_brief_builds_one_document_with_editable_layers() {
        let mut s = session();
        let answer = build(&mut s, &json!({ "brief": brief() })).expect("built");
        assert_eq!(s.documents().len(), 1);
        let layers = answer["layers"].as_u64().expect("a layer count");
        let doc = s.active().expect("a document");
        assert_eq!(doc.doc.layers.len() as u64, layers);
        assert!(layers >= 6, "{layers} layers");
        assert_eq!(doc.doc.size.width, 2480);
        assert_eq!(doc.doc.size.height, 3508);
        assert_eq!(doc.doc.name, "Autumn Print Fair");
        // The words are real type layers, and the panels real shape layers.
        assert!(doc.doc.layers.iter().any(|layer| layer.name == "Headline"));
        assert!(doc.doc.layers.iter().any(|layer| layer.name == "Accent Rule"));
    }

    #[test]
    fn a_built_poster_arrives_clean_and_with_nothing_to_undo() {
        let mut s = session();
        build(&mut s, &json!({ "brief": brief() })).expect("built");
        let doc = s.active().expect("a document");
        assert!(!doc.is_dirty(), "a finished poster is not a modified document");
        assert!(!doc.history.can_undo(), "the build's steps are not undo steps");
    }

    #[test]
    fn a_bad_brief_leaves_the_open_documents_alone() {
        let mut s = session();
        build(&mut s, &json!({ "brief": brief() })).expect("built");
        let before = s.documents().len();
        for bad_brief in [
            json!({}),
            json!({ "brief": null }),
            json!({ "brief": [1, 2, 3] }),
            json!({ "brief": { "title": "" } }),
            json!({ "brief": { "title": "T", "format": "nope" } }),
            json!({ "brief": { "title": "T", "template": "spiral" } }),
            json!({ "brief": { "title": "T", "version": 99 } }),
        ] {
            assert!(build(&mut s, &bad_brief).is_err(), "{bad_brief}");
            assert_eq!(s.documents().len(), before, "{bad_brief} left a document behind");
        }
        assert!(s.active().is_some());
    }

    #[test]
    fn planning_changes_nothing_and_describes_the_layers() {
        let mut s = session();
        let answer = plan(&mut s, &json!({ "brief": brief() })).expect("planned");
        assert!(s.documents().is_empty());
        assert_eq!(answer["template"], "stack");
        let steps = answer["steps"].as_array().expect("steps");
        assert!(steps.len() >= 6);
        assert_eq!(steps[0]["command"], "file.new");
        let names = answer["names"].as_array().expect("names");
        assert!(names.iter().any(|name| name == "Headline"));
    }

    #[test]
    fn every_template_builds() {
        for template in photocraft_ai::template::IDS {
            let mut s = session();
            let mut brief_with_template = brief();
            brief_with_template["template"] = json!(template);
            let answer = build(&mut s, &json!({ "brief": brief_with_template, "name": format!("Poster {template}") })).expect("built");
            assert_eq!(answer["template"], *template);
            assert!(answer["layers"].as_u64().unwrap_or(0) >= 3, "{template}");
        }
    }

    #[test]
    fn two_posters_are_two_documents() {
        let mut s = session();
        build(&mut s, &json!({ "brief": brief() })).expect("built");
        let mut other = brief();
        other["title"] = json!("Night Market");
        other["format"] = json!("square");
        other["template"] = json!("hero");
        build(&mut s, &json!({ "brief": other })).expect("built");
        assert_eq!(s.documents().len(), 2);
    }

    #[test]
    fn presets_lists_what_the_panel_needs() {
        let mut s = session();
        let answer = presets(&mut s, &json!({})).expect("listed");
        assert!(answer["presets"].as_array().is_some_and(|list| list.len() >= 5));
        assert!(answer["templates"].as_array().is_some_and(|list| list.len() == photocraft_ai::template::IDS.len()));
        let schema = answer["brief"].as_str().expect("a schema");
        assert!(schema.contains("\"title\""));
    }

    #[test]
    fn hostile_brief_parameters_are_errors_not_crashes() {
        let mut s = session();
        for params in
            [json!(null), json!(0), json!("brief"), json!([]), json!({ "brief": { "title": 42 } }), json!({ "brief": { "title": "T", "size": "big" } })]
        {
            assert!(build(&mut s, &params).is_err(), "{params}");
            assert!(plan(&mut s, &params).is_err(), "{params}");
        }
        assert!(s.documents().is_empty());
    }

    #[test]
    fn the_specs_are_registered_under_their_ids() {
        let specs = specs();
        for id in [PLAN, BUILD, PRESETS_CMD, CREATE, STATUS, CANCEL] {
            assert!(specs.iter().any(|spec| spec.id == id), "{id} is missing");
        }
        for spec in &specs {
            assert!(!spec.params.is_empty(), "{} documents its parameters", spec.id);
            assert!(!spec.journal, "{} must not journal a prompt or an answer", spec.id);
        }
    }
}
