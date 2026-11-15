//! Window › AI › Create Poster: a prompt becomes an editable poster.
//!
//! The panel never builds a document itself. It asks the engine for a brief
//! (`ai.poster.create`), shows what the answer will become, and runs `ai.poster.build` when the
//! person says so. Every command the panel runs is one an agent can run too, with the same ids and
//! the same parameters, so the window is a view of the engine rather than a second implementation.
//!
//! Two things deliberately live here rather than in the engine:
//!
//! - **The model call.** It needs a socket, and the engine has none. [`plan_fn`] wraps the app's
//!   [`Planner`] in the background seam `Session::ai` already understands, so `ai.poster.create`
//!   returns at once and `ai.poster.status` reports the answer.
//! - **The provider settings.** An API key must never be a command parameter: parameters are
//!   journaled, replayed and readable through the control channel. The key stays in
//!   [`Settings`], shared with the plan function and written to the app's config file by the
//!   platform, and [`Runtime`] never puts it in `ui.ron`, a document or a status line.
//!
//! The preview is painted from the plan's own steps, not from a second model: the paper, every
//! fill, shape and type box the build will create. What is shown is what will be built.

use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

use photocraft_ai::{Planner, Request, Settings};
use photocraft_engine::ai_cmds::{PlanFn, PosterStatus};

use crate::PhotocraftApp;
use crate::theme::Tokens;

/// The panel's width, in points.
const WIDTH: f32 = 360.0;
/// Longest prompt the panel accepts; the engine refuses anything longer.
const MAX_PROMPT: usize = photocraft_ai::provider::MAX_PROMPT;
/// Mood hints offered as chips. Any word is accepted by the engine; these are the usual ones.
const MOODS: &[&str] = &["warm", "minimal", "bold", "mono", "playful", "elegant"];

/// What the app installs: where the provider settings live, and how a planner is built from them.
///
/// `settings` is shared with [`plan_fn`]'s worker, so saving in the panel changes the next request
/// without a restart or a command. `make` is the app's transport: a Rust HTTP client on desktop,
/// Android's stack through its Java bridge on a tablet.
pub struct Service {
    pub settings: Arc<Mutex<Settings>>,
    pub make: Arc<MakePlanner>,
    /// Persist the settings text (the app's config file, or the platform's store).
    ///
    /// Spelled out rather than reusing `services::SaveTextFn`, so a public type in a public module
    /// does not name a type whose module is crate-private.
    pub save: Box<SaveSettings>,
}

/// Builds a planner from the settings: the app's transport, and the one place a socket is opened.
pub type MakePlanner = dyn Fn(&Settings) -> Result<Box<dyn Planner + Send>, String> + Send + Sync;

/// Writes the provider settings to the app's own store (never to `ui.ron` or a document).
pub type SaveSettings = dyn FnMut(&str) -> Result<(), String>;

/// The engine's background seam, over the app's planner.
///
/// Settings are checked before a thread starts, so "no model is set" is an immediate `Err` the
/// panel shows beside the buttons, not a failed request a person waits on. A planner that cannot
/// be built (an unsupported endpoint) is the same. The answer is validated by the planner before
/// it is sent back, so a bad brief arrives as a sentence.
pub fn plan_fn(service: &Service) -> PlanFn {
    let settings = service.settings.clone();
    let make = service.make.clone();
    Box::new(move |request: &Request| {
        request.validate().map_err(|error| error.to_string())?;
        let current = settings.lock().map(|settings| settings.clone()).unwrap_or_else(|poisoned| poisoned.into_inner().clone());
        current.validate().map_err(|error| error.to_string())?;
        let planner = make(&current)?;
        let request = request.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("photocraft-ai".into())
            .spawn(move || {
                // A dropped receiver means Cancel: the answer is discarded, not an error to report.
                let _ = tx.send(planner.brief(&request).map_err(|error| error.to_string()));
            })
            .map_err(|error| format!("Could not start the AI request: {error}"))?;
        Ok(rx)
    })
}

/// What the panel keeps between frames. Nothing here is persisted: a prompt is the person's, and
/// the key is not written to `ui.ron`, a document or the journal.
#[derive(Default)]
pub(crate) struct Runtime {
    /// The settings shared with the plan function; `None` in a frontend without a model call.
    settings: Option<Arc<Mutex<Settings>>>,
    save: Option<Box<SaveSettings>>,
    /// The settings as the panel edits them, before Save.
    draft: Settings,
    /// The key as typed. The field masks it; it is only readable here and in `draft`.
    key: String,
    /// The prompt and the choices.
    prompt: String,
    format: String,
    template: String,
    mood: Vec<String>,
    /// The last answer, from `ai.poster.create` or `ai.poster.plan`.
    answer: Option<Value>,
    /// A message beside the buttons: a refused start, or what Save came to.
    message: Option<String>,
    /// Whether the provider settings are expanded.
    settings_open: bool,
}

impl Runtime {
    pub(crate) fn new(service: Service) -> Self {
        let draft = service.settings.lock().map(|settings| settings.clone()).unwrap_or_default();
        let key = draft.key.clone().unwrap_or_default();
        Self { settings: Some(service.settings), save: Some(service.save), draft, key, ..Self::default() }
    }

    /// The request `Create` sends: the prompt, and only the choices that were made.
    fn create_params(&self) -> Value {
        let mut params = serde_json::Map::new();
        params.insert("prompt".into(), json!(self.prompt));
        if !self.format.is_empty() {
            params.insert("format".into(), json!(self.format));
        }
        if !self.template.is_empty() {
            params.insert("template".into(), json!(self.template));
        }
        if !self.mood.is_empty() {
            params.insert("style".into(), json!(self.mood));
        }
        Value::Object(params)
    }

    /// Save the provider settings: check them, hand the text to the platform, then publish them to
    /// the next request. A failure leaves the previous settings in force.
    fn save_settings(&mut self) -> Result<(), String> {
        self.draft.key = match self.key.trim() {
            "" => None,
            key => Some(key.trim().to_string()),
        };
        // An empty base URL is a fresh install, not an error: the panel says "not set up" and
        // Create reports it. A URL that was typed, though, must be usable.
        if !self.draft.base_url.trim().is_empty() {
            self.draft.validate().map_err(|error| error.to_string())?;
        }
        let text = self.draft.to_json().map_err(|error| error.to_string())?;
        if let Some(save) = self.save.as_mut() {
            save(&text)?;
        }
        if let Some(settings) = &self.settings
            && let Ok(mut live) = settings.lock()
        {
            *live = self.draft.clone();
        }
        Ok(())
    }
}

/// One frame's answer from the panel: run it after the window's closure, so the borrow of `app`
/// inside the closure ends first.
enum Action {
    Create,
    Build,
    Cancel,
    SaveSettings,
    Close,
}

/// Follow the background request and load the provider settings once.
pub(crate) fn tick(app: &mut PhotocraftApp, ctx: &egui::Context) {
    if app.session.ai.poll()
        && let PosterStatus::Ready { answer } = &app.session.ai.status
    {
        app.ai_rt.answer = Some(answer.clone());
        // The answer is the point of the panel: bring it back if it was closed while waiting.
        app.ui.ai_open = true;
    }
    if app.session.ai.busy() {
        ctx.request_repaint_after(std::time::Duration::from_millis(120));
    }
}

/// The panel window, when it is open.
pub(crate) fn show(app: &mut PhotocraftApp, ctx: &egui::Context) {
    if !app.ui.ai_open {
        return;
    }
    let t = Tokens::get(ctx);
    let canvas = app.last_canvas_rect;
    let mut action: Option<Action> = None;
    egui::Window::new(crate::i18n::t("Create Poster"))
        .id(egui::Id::unique("ai-create-poster"))
        .title_bar(false)
        .resizable(false)
        .collapsible(false)
        .frame(float_frame(&t))
        .default_pos(egui::pos2((canvas.center().x - WIDTH / 2.0).max(canvas.left() + 8.0), canvas.top() + 40.0))
        .default_width(WIDTH)
        .show(ctx, |ui| {
            ui.set_width(WIDTH);
            if crate::analysis_ui::title_row(ui, "Create Poster") {
                action = Some(Action::Close);
            }
            prompt(ui, app, &t);
            ui.add_space(6.0);
            choices(ui, app, &t);
            ui.add_space(6.0);
            crate::widgets::hairline(ui);
            ui.add_space(6.0);
            answer(ui, app, &t);
            ui.add_space(6.0);
            crate::widgets::hairline(ui);
            ui.add_space(6.0);
            provider(ui, app, &t, &mut action);
            ui.add_space(6.0);
            crate::widgets::hairline(ui);
            ui.add_space(6.0);
            buttons(ui, app, &t, &mut action);
        });
    if let Some(action) = action {
        run(app, action);
    }
}

fn run(app: &mut PhotocraftApp, action: Action) {
    match action {
        Action::Close => app.ui.ai_open = false,
        Action::Create => {
            let params = app.ai_rt.create_params();
            app.ai_rt.message = app.run("ai.poster.create", params).err();
        }
        Action::Build => {
            let params = app.ai_rt.answer.as_ref().and_then(|answer| answer.get("params").cloned());
            let Some(params) = params else {
                app.ai_rt.message = Some("Ask for a poster first".into());
                return;
            };
            match app.run("ai.poster.build", params) {
                Ok(_) => {
                    app.ai_rt.message = None;
                    // The document is the point: get the panel out of its way.
                    app.ui.ai_open = false;
                }
                Err(error) => app.ai_rt.message = Some(error),
            }
        }
        Action::Cancel => {
            app.ai_rt.message = app.run("ai.poster.cancel", json!({})).err();
        }
        Action::SaveSettings => {
            app.ai_rt.message = match app.ai_rt.save_settings() {
                Ok(()) => Some("Saved. The next request uses these settings.".into()),
                Err(error) => Some(error),
            };
        }
    }
}

/// The prompt, with a character count once it gets close to the limit.
fn prompt(ui: &mut egui::Ui, app: &mut PhotocraftApp, t: &Tokens) {
    crate::widgets::section_label(ui, "Prompt");
    let mut text = std::mem::take(&mut app.ai_rt.prompt);
    let edit = egui::TextEdit::multiline(&mut text)
        .desired_rows(4)
        .desired_width(f32::INFINITY)
        .hint_text(crate::i18n::t("A poster for a night market: ceramics, live music, Saturday"))
        .font(egui::TextStyle::Body);
    let response = ui.add(edit);
    // Cap the prompt here as well as in the engine, so the field cannot grow without bound.
    if text.chars().count() > MAX_PROMPT {
        text = text.chars().take(MAX_PROMPT).collect();
    }
    app.ai_rt.prompt = text;
    if app.ai_rt.prompt.chars().count() > MAX_PROMPT - 400 {
        let count = app.ai_rt.prompt.chars().count();
        ui.label(egui::RichText::new(format!("{count} / {MAX_PROMPT}")).color(t.text_faint).size(10.5));
    }
    let _ = response;
}

/// Page, layout and mood.
fn choices(ui: &mut egui::Ui, app: &mut PhotocraftApp, t: &Tokens) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(crate::i18n::t("Page")).color(t.text_dim).size(11.5));
        let mut pages: Vec<(String, &str)> = vec![(String::new(), crate::i18n::t("Auto"))];
        pages.extend(photocraft_ai::PRESETS.iter().map(|preset| (preset.id.to_string(), preset.label)));
        let mut current = app.ai_rt.format.clone();
        if crate::widgets::dropdown(ui, "ai-page", "Page", &mut current, &pages, 150.0) {
            app.ai_rt.format = current;
        }
        ui.label(egui::RichText::new(crate::i18n::t("Layout")).color(t.text_dim).size(11.5));
        let mut layouts: Vec<(String, &str)> = vec![(String::new(), crate::i18n::t("Auto"))];
        layouts.extend(photocraft_ai::template::IDS.iter().map(|id| (id.to_string(), *id)));
        let mut current = app.ai_rt.template.clone();
        if crate::widgets::dropdown(ui, "ai-layout", "Layout", &mut current, &layouts, 110.0) {
            app.ai_rt.template = current;
        }
    });
    ui.add_space(4.0);
    ui.horizontal_wrapped(|ui| {
        ui.label(egui::RichText::new(crate::i18n::t("Mood")).color(t.text_dim).size(11.5));
        for word in MOODS {
            let selected = app.ai_rt.mood.iter().any(|chosen| chosen == word);
            if crate::widgets::pill_tab(ui, word, selected).clicked() {
                if selected {
                    app.ai_rt.mood.retain(|chosen| chosen != word);
                } else {
                    app.ai_rt.mood.push((*word).to_string());
                }
            }
        }
    });
}

/// The preview, the brief and the status line.
fn answer(ui: &mut egui::Ui, app: &mut PhotocraftApp, t: &Tokens) {
    match app.ai_rt.answer.clone() {
        Some(answer) => {
            preview(ui, &answer, WIDTH - 16.0);
            ui.add_space(4.0);
            summary(ui, &answer, t);
        }
        None => {
            ui.label(egui::RichText::new(crate::i18n::t("Nothing yet: describe the poster and press Create.")).color(t.text_faint).size(11.5));
        }
    }
    ui.add_space(4.0);
    let (text, color) = match &app.session.ai.status {
        PosterStatus::Idle => (crate::i18n::t("Ready when you are.").to_string(), t.text_faint),
        PosterStatus::Planning => (crate::i18n::t("Asking the model…").to_string(), t.accent_text),
        PosterStatus::Ready { answer } => {
            let layers = answer.get("layers").and_then(Value::as_u64).unwrap_or(0);
            (format!("{} {layers} {}", crate::i18n::t("Ready:"), crate::i18n::t("layers")), t.text)
        }
        PosterStatus::Failed { message } => (message.clone(), t.danger),
    };
    ui.label(egui::RichText::new(text).color(color).size(11.5));
    if let Some(message) = &app.ai_rt.message {
        ui.label(egui::RichText::new(message).color(t.danger).size(11.5));
    }
    if app.session.ai.plan.is_none() {
        ui.label(
            egui::RichText::new(crate::i18n::t("This build has no model connection: the plan and build commands still work.")).color(t.text_faint).size(11.0),
        );
    }
}

/// The provider settings, collapsed until opened.
fn provider(ui: &mut egui::Ui, app: &mut PhotocraftApp, t: &Tokens, action: &mut Option<Action>) {
    // The disclosure chevron is an icon, not a text arrow: the app's fonts have no `▸`/`▾`.
    let arrow = if app.ai_rt.settings_open { "chevron-down" } else { "chevron-right" };
    let icon = crate::icons::image(arrow, 10.0, t.text_dim);
    ui.horizontal(|ui| {
        let label = egui::RichText::new(crate::i18n::t("Provider")).color(t.text_dim).size(11.5);
        if ui.add(egui::Button::image_and_text(icon, label).frame(false)).clicked() {
            app.ai_rt.settings_open = !app.ai_rt.settings_open;
        }
        ui.label(egui::RichText::new(app.ai_rt.draft.describe()).color(t.text_faint).size(11.0));
    });
    if !app.ai_rt.settings_open {
        return;
    }
    ui.add_space(2.0);
    field(ui, t, "Base URL", &mut app.ai_rt.draft.base_url, "https://api.openai.com/v1", false);
    field(ui, t, "Model", &mut app.ai_rt.draft.model, "gpt-4o-mini", false);
    field(ui, t, "API key", &mut app.ai_rt.key, "leave empty for a local server", true);
    ui.horizontal(|ui| {
        if ui.button(crate::i18n::t("Save")).clicked() {
            *action = Some(Action::SaveSettings);
        }
        ui.label(egui::RichText::new(crate::i18n::t("Kept in this app's settings file, not in the document.")).color(t.text_faint).size(10.5));
    });
}

fn field(ui: &mut egui::Ui, t: &Tokens, label: &str, value: &mut String, hint: &str, secret: bool) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(crate::i18n::t(label)).color(t.text_dim).size(11.5));
        let mut edit = egui::TextEdit::singleline(value).desired_width(220.0).hint_text(crate::i18n::t(hint)).font(egui::TextStyle::Body);
        if secret {
            edit = edit.password(true);
        }
        ui.add(edit);
    });
}

/// Create, Build and Cancel.
fn buttons(ui: &mut egui::Ui, app: &mut PhotocraftApp, t: &Tokens, action: &mut Option<Action>) {
    let busy = app.session.ai.busy();
    let can_plan = app.session.ai.plan.is_some();
    let ready = app.ai_rt.answer.is_some() && !busy;
    ui.horizontal(|ui| {
        let create = ui.add_enabled(
            can_plan && !busy && !app.ai_rt.prompt.trim().is_empty(),
            egui::Button::new(egui::RichText::new(crate::i18n::t("Create")).color(t.primary_text).size(12.0).strong())
                .fill(t.primary_bg)
                .stroke(egui::Stroke::NONE),
        );
        if create.clicked() {
            *action = Some(Action::Create);
        }
        if ui.add_enabled(ready, egui::Button::new(crate::i18n::t("Build into a new document"))).clicked() {
            *action = Some(Action::Build);
        }
        if busy && ui.button(crate::i18n::t("Cancel")).clicked() {
            *action = Some(Action::Cancel);
        }
    });
}

fn float_frame(t: &Tokens) -> egui::Frame {
    egui::Frame::NONE
        .fill(t.card)
        .stroke(egui::Stroke::new(1.0, t.card_border))
        .corner_radius(egui::CornerRadius::same(t.radius_lg as u8))
        .shadow(egui::Shadow { offset: [0, 10], blur: 30, spread: 0, color: t.shadow })
        .inner_margin(egui::Margin::same(8))
}

/// The plan as a wireframe: the paper, then every fill, shape and type box in build order.
///
/// This is painted from the same steps `ai.poster.build` runs, so the preview cannot drift from
/// the document: a step that draws nothing here would draw nothing there either. It is a
/// wireframe, not a render — type is approximated with the panel's own font.
fn preview(ui: &mut egui::Ui, answer: &Value, width: f32) {
    let doc_w = answer.get("width").and_then(Value::as_f64).unwrap_or(1.0).max(1.0) as f32;
    let doc_h = answer.get("height").and_then(Value::as_f64).unwrap_or(1.0).max(1.0) as f32;
    let dpi = answer.get("dpi").and_then(Value::as_f64).unwrap_or(72.0).clamp(1.0, 2400.0) as f32;
    let height = (width * doc_h / doc_w).clamp(60.0, 300.0);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 4.0, egui::Color32::from_gray(28));
    let scale = (width / doc_w).min(height / doc_h);
    let paper = egui::Rect::from_center_size(rect.center(), egui::vec2(doc_w * scale, doc_h * scale));
    let paper_fill = answer.get("paper").and_then(Value::as_str).and_then(hex_color).unwrap_or(egui::Color32::WHITE);
    painter.rect_filled(paper, 1.0, paper_fill);
    let ink = answer
        .get("brief")
        .and_then(|brief| brief.get("palette"))
        .and_then(|palette| palette.get("ink"))
        .and_then(Value::as_str)
        .and_then(hex_color)
        .unwrap_or(egui::Color32::BLACK);
    let to_screen = |x: f32, y: f32, w: f32, h: f32| {
        egui::Rect::from_min_size(paper.min + egui::vec2(x * scale, y * scale), egui::vec2((w * scale).max(0.5), (h * scale).max(0.5)))
    };
    let Some(steps) = answer.get("steps").and_then(Value::as_array) else { return };
    for step in steps {
        let command = step.get("command").and_then(Value::as_str).unwrap_or_default();
        let params = step.get("params").unwrap_or(&Value::Null);
        match command {
            "layer.newFillLayer.solidColor" => {
                if let Some(color) = params.get("color").and_then(Value::as_str).and_then(hex_color) {
                    painter.rect_filled(paper, 0.0, color);
                }
            }
            "layer.newFillLayer.gradient" => gradient(&painter, paper, params),
            "shape.create" => {
                let Some([x, y, w, h]) = rect_param(params.get("rect")) else { continue };
                let screen = to_screen(x, y, w, h);
                let radius = (params.get("radii").and_then(Value::as_f64).unwrap_or(0.0) as f32 * scale).max(0.0);
                if let Some(fill) = params.get("fill").and_then(Value::as_str).and_then(hex_color) {
                    painter.rect_filled(screen, radius, fill);
                }
                if let Some(stroke) = params.get("stroke") {
                    let color = stroke.get("color").and_then(Value::as_str).and_then(hex_color).unwrap_or(ink);
                    let width = (stroke.get("width").and_then(Value::as_f64).unwrap_or(1.0) as f32 * scale).max(0.5);
                    painter.rect_stroke(screen, radius, egui::Stroke::new(width, color), egui::StrokeKind::Inside);
                }
            }
            "type.create" => {
                let Some([x, y, w, h]) = rect_param(params.get("box")) else { continue };
                let screen = to_screen(x, y, w, h);
                let color = params.get("color").and_then(Value::as_str).and_then(hex_color).unwrap_or(ink);
                let points = params.get("size").and_then(Value::as_f64).unwrap_or(10.0) as f32;
                // Points to pixels at the document's resolution, then to the preview's scale.
                let size = (points * dpi / 72.0 * scale).clamp(3.0, 40.0);
                let mut text = params.get("text").and_then(Value::as_str).unwrap_or_default().to_string();
                if params.get("caps").and_then(Value::as_str) == Some("all") {
                    text = text.to_uppercase();
                }
                if text.is_empty() {
                    continue;
                }
                let galley = painter.layout(text, egui::FontId::proportional(size), color, screen.width().max(1.0));
                let pos = match params.get("align").and_then(Value::as_str) {
                    Some("center") => egui::pos2(screen.center().x - galley.size().x / 2.0, screen.top()),
                    Some("right") => egui::pos2(screen.right() - galley.size().x, screen.top()),
                    _ => screen.left_top(),
                };
                painter.with_clip_rect(screen).galley(pos, galley, color);
            }
            _ => {}
        }
    }
    painter.rect_stroke(paper, 1.0, egui::Stroke::new(1.0, egui::Color32::from_black_alpha(70)), egui::StrokeKind::Inside);
}

/// A two-colour linear fill, as four vertices egui interpolates.
fn gradient(painter: &egui::Painter, rect: egui::Rect, params: &Value) {
    let from = params.get("from").and_then(Value::as_str).and_then(hex_color);
    let to = params.get("to").and_then(Value::as_str).and_then(hex_color);
    let (Some(from), Some(to)) = (from, to) else { return };
    let angle = params.get("angle").and_then(Value::as_f64).unwrap_or(0.0).to_radians();
    let (dx, dy) = (angle.cos() as f32, angle.sin() as f32);
    let corners = [
        (rect.left_top(), -0.5 * dx - 0.5 * dy),
        (rect.right_top(), 0.5 * dx - 0.5 * dy),
        (rect.right_bottom(), 0.5 * dx + 0.5 * dy),
        (rect.left_bottom(), -0.5 * dx + 0.5 * dy),
    ];
    let lo = corners.iter().map(|(_, t)| *t).fold(f32::MAX, f32::min);
    let hi = corners.iter().map(|(_, t)| *t).fold(f32::MIN, f32::max);
    let span = (hi - lo).max(1e-4);
    let mut mesh = egui::Mesh::default();
    for (pos, at) in corners {
        mesh.colored_vertex(pos, lerp_color(from, to, ((at - lo) / span).clamp(0.0, 1.0)));
    }
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    painter.add(egui::Shape::mesh(mesh));
}

fn lerp_color(from: egui::Color32, to: egui::Color32, t: f32) -> egui::Color32 {
    let mix = |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * t).round().clamp(0.0, 255.0) as u8;
    egui::Color32::from_rgb(mix(from.r(), to.r()), mix(from.g(), to.g()), mix(from.b(), to.b()))
}

fn hex_color(text: &str) -> Option<egui::Color32> {
    let hex = text.strip_prefix('#')?;
    if hex.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(hex.get(0..2)?, 16).ok()?;
    let g = u8::from_str_radix(hex.get(2..4)?, 16).ok()?;
    let b = u8::from_str_radix(hex.get(4..6)?, 16).ok()?;
    Some(egui::Color32::from_rgb(r, g, b))
}

fn rect_param(value: Option<&Value>) -> Option<[f32; 4]> {
    let items = value?.as_array()?;
    if items.len() != 4 {
        return None;
    }
    let mut out = [0.0f32; 4];
    for (slot, item) in out.iter_mut().zip(items) {
        let number = item.as_f64()?;
        if !number.is_finite() {
            return None;
        }
        *slot = number.clamp(-1.0e6, 1.0e6) as f32;
    }
    Some(out)
}

/// The brief's words, the page and the palette, under the preview.
fn summary(ui: &mut egui::Ui, answer: &Value, t: &Tokens) {
    let brief = answer.get("brief").unwrap_or(&Value::Null);
    if let Some(title) = brief.get("title").and_then(Value::as_str) {
        ui.label(egui::RichText::new(title).color(t.text).size(12.5).strong());
    }
    let mut line = Vec::new();
    if let Some(template) = answer.get("template").and_then(Value::as_str) {
        line.push(template.to_string());
    }
    if let Some(size) = answer.get("size").and_then(Value::as_str) {
        line.push(size.to_string());
    }
    if let Some(layers) = answer.get("layers").and_then(Value::as_u64) {
        line.push(format!("{layers} {}", crate::i18n::t("layers")));
    }
    if !line.is_empty() {
        ui.label(egui::RichText::new(line.join(" · ")).color(t.text_dim).size(11.0));
    }
    let palette = brief.get("palette").unwrap_or(&Value::Null);
    ui.horizontal(|ui| {
        for key in ["background", "ink", "accent"] {
            let Some(hex) = palette.get(key).and_then(Value::as_str) else { continue };
            let Some(color) = hex_color(hex) else { continue };
            let (rect, response) = ui.allocate_exact_size(egui::vec2(18.0, 14.0), egui::Sense::hover());
            ui.painter().rect_filled(rect, 2.0, color);
            ui.painter().rect_stroke(rect, 2.0, egui::Stroke::new(1.0, t.card_border), egui::StrokeKind::Inside);
            response.on_hover_text(hex);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Services, menus};
    use photocraft_ai::Scripted;
    use photocraft_engine::Session;

    fn brief() -> Value {
        json!({
            "title": "Autumn Print Fair",
            "subtitle": "Twelve studios open late",
            "body": "Prints, posters and new work.",
            "footer": "Sat 14 Nov · 6-9 pm",
            "format": "a4",
        })
    }

    /// A service whose planner answers with `brief()`, and a settings handle a test can read.
    fn service() -> (Service, Arc<Mutex<Settings>>, Arc<Mutex<u32>>) {
        let settings =
            Arc::new(Mutex::new(Settings { base_url: "https://api.example.test/v1".into(), model: "gpt-4o-mini".into(), key: Some("sk-test".into()) }));
        let saves = Arc::new(Mutex::new(0));
        let count = saves.clone();
        let service = Service {
            settings: settings.clone(),
            make: Arc::new(|_| Ok(Box::new(Scripted::new(brief()).labelled("test")))),
            save: Box::new(move |_| {
                *count.lock().unwrap() += 1;
                Ok(())
            }),
        };
        (service, settings, saves)
    }

    fn app_with_ai() -> (PhotocraftApp, Arc<Mutex<Settings>>, Arc<Mutex<u32>>) {
        let (service, settings, saves) = service();
        let app = PhotocraftApp::new(Session::new(), Services { ai: Some(service), ..Default::default() });
        (app, settings, saves)
    }

    #[test]
    fn a_planner_starts_a_worker_and_the_engine_reports_the_brief() {
        let (service, _, _) = service();
        let mut plan = plan_fn(&service);
        let rx = plan(&Request::new("an autumn print fair")).expect("started");
        let brief = rx.recv_timeout(std::time::Duration::from_secs(5)).expect("an answer").expect("a brief");
        assert_eq!(brief.title, "Autumn Print Fair");
    }

    #[test]
    fn settings_that_are_not_set_up_fail_before_a_thread_starts() {
        let (mut service, settings, _) = service();
        *settings.lock().unwrap() = Settings::default();
        service.settings = settings.clone();
        let mut plan = plan_fn(&service);
        let error = plan(&Request::new("a poster")).expect_err("not set up").to_string();
        assert!(error.contains("base URL") || error.contains("model"), "{error}");
        assert!(!error.contains("sk-test"), "a refusal must not echo the key: {error}");
        // An empty prompt is refused too, and says which field.
        let (service, _, _) = self::service();
        let mut plan = plan_fn(&service);
        assert!(plan(&Request::new("   ")).is_err());
    }

    #[test]
    fn the_panel_opens_from_the_window_menu_and_builds_a_document() {
        let (mut app, _, _) = app_with_ai();
        let ctx = egui::Context::default();
        assert!(menus::is_enabled(&app, "window.ai.createPoster"));
        menus::invoke(&mut app, &ctx, "window.ai.createPoster", json!({})).expect("opens");
        assert!(app.ui.ai_open);

        app.ai_rt.prompt = "an autumn print fair".into();
        app.ai_rt.format = "a4".into();
        run(&mut app, Action::Create);
        assert!(app.ai_rt.message.is_none(), "{:?}", app.ai_rt.message);
        // The answer arrives on the background seam, then the preview is the same shape a plan is.
        for _ in 0..400 {
            tick(&mut app, &ctx);
            if app.ai_rt.answer.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let answer = app.ai_rt.answer.clone().expect("an answer");
        assert_eq!(answer["template"], "stack");
        assert!(answer["steps"].as_array().is_some_and(|steps| steps.len() >= 6));

        run(&mut app, Action::Build);
        assert_eq!(app.session.documents().len(), 1, "{:?}", app.ai_rt.message);
        assert!(!app.ui.ai_open, "the panel gets out of the document's way");
        assert_eq!(app.session.active().map(|state| state.doc.name.as_str()), Some("Autumn Print Fair"));
    }

    #[test]
    fn a_panel_without_a_planner_says_so_and_cannot_create() {
        let mut app = PhotocraftApp::new(Session::new(), Services::default());
        let ctx = egui::Context::default();
        assert!(!menus::is_enabled(&app, "window.ai.createPoster"));
        // The command still answers, so an agent can tell "no planner" from "no such command".
        let status = app.run("ai.poster.status", json!({})).expect("status");
        assert_eq!(status["supported"], false);
        app.ui.ai_open = true;
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| show(&mut app, ui.ctx()));
        output.textures_delta.clear();
        assert!(app.ui.ai_open, "the panel still explains itself");
    }

    #[test]
    fn the_key_is_masked_and_saving_publishes_the_settings() {
        let (mut app, settings, saves) = app_with_ai();
        assert_eq!(app.ai_rt.key, "sk-test", "the field starts with the saved key");
        app.ai_rt.key = " sk-new ".into();
        app.ai_rt.draft.model = " gpt-4.1 ".into();
        run(&mut app, Action::SaveSettings);
        assert!(app.ai_rt.message.as_deref().is_some_and(|message| message.starts_with("Saved")), "{:?}", app.ai_rt.message);
        assert_eq!(*saves.lock().unwrap(), 1);
        let live = settings.lock().unwrap().clone();
        assert_eq!(live.key.as_deref(), Some("sk-new"), "trimmed");
        assert_eq!(live.model, " gpt-4.1 ");
        assert!(!format!("{:?}", app.ai_rt.draft).contains("sk-new"), "Debug stays redacted");

        // A blank key clears it: a local server wants none.
        app.ai_rt.key = String::new();
        run(&mut app, Action::SaveSettings);
        assert!(settings.lock().unwrap().key.is_none());

        // A URL that cannot be used is refused, and the old settings stay in force.
        app.ai_rt.draft.base_url = "http://api.example.test/v1".into();
        run(&mut app, Action::SaveSettings);
        assert!(app.ai_rt.message.as_deref().is_some_and(|message| message.contains("cleartext")), "{:?}", app.ai_rt.message);
        assert_eq!(*saves.lock().unwrap(), 2, "nothing was written");
    }

    #[test]
    fn ui_set_opens_and_closes_the_panel() {
        let (mut app, _, _) = app_with_ai();
        let ctx = egui::Context::default();
        let call = |app: &mut PhotocraftApp, params: Value| {
            let (req, _) = crate::control::ControlRequest::new("ui.set", params);
            match crate::control::handle(app, &ctx, &req) {
                crate::control::Outcome::Done(value) => value,
                _ => panic!("expected an immediate reply"),
            }
        };
        assert_eq!(call(&mut app, json!({"aiOpen": true}))["ok"], true);
        assert!(app.ui.ai_open);
        let (req, _) = crate::control::ControlRequest::new("ui.inspect", json!({}));
        let inspected = match crate::control::handle(&mut app, &ctx, &req) {
            crate::control::Outcome::Done(value) => value,
            _ => panic!("expected an immediate reply"),
        };
        assert_eq!(inspected["result"]["aiOpen"], true);
        assert_eq!(call(&mut app, json!({"aiOpen": false, "maskTarget": "bad"}))["ok"], false);
        assert!(app.ui.ai_open, "a bad field changes nothing");
        assert_eq!(call(&mut app, json!({"aiOpen": false}))["ok"], true);
        assert!(!app.ui.ai_open);
    }

    #[test]
    fn the_preview_paints_every_template_and_survives_a_broken_answer() {
        let ctx = egui::Context::default();
        for template in photocraft_ai::template::IDS {
            let brief = photocraft_ai::Brief::from_json(&{
                let mut brief = brief();
                brief["template"] = json!(template);
                brief
            })
            .expect("a brief");
            let answer = json!({
                "brief": serde_json::to_value(&brief).expect("JSON"),
                "params": {},
                "template": template,
                "size": "A4",
                "paper": "#ffffff",
                "width": 2480,
                "height": 3508,
                "dpi": 300,
                "layers": 6,
                "steps": photocraft_ai::plan::steps(&photocraft_ai::template::compose(&brief).expect("a design"), "P")
                    .iter()
                    .map(photocraft_ai::Step::to_json)
                    .collect::<Vec<_>>(),
            });
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| preview(ui, &answer, 300.0));
            output.textures_delta.clear();
        }
        // A hostile answer is drawn as nothing, never as a panic.
        for broken in [
            json!({}),
            json!({"width": 0, "height": 0, "steps": [{"command": "type.create", "params": {"box": [0, 0], "text": "x"}}]}),
            json!({"width": -5, "height": 1e300, "steps": [{"command": "shape.create", "params": {"rect": ["a", null, 1, 2]}}]}),
            json!({"steps": [{"command": "layer.newFillLayer.gradient", "params": {"from": "#zzz", "to": 7}}]}),
            json!({"steps": "not an array"}),
        ] {
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| preview(ui, &broken, 300.0));
            output.textures_delta.clear();
        }
    }

    #[test]
    fn create_and_build_need_the_choices_they_say_they_need() {
        let (mut app, _, _) = app_with_ai();
        app.ai_rt.prompt = "a poster".into();
        app.ai_rt.format = "a3".into();
        app.ai_rt.template = "hero".into();
        app.ai_rt.mood = vec!["warm".into(), "bold".into()];
        let params = app.ai_rt.create_params();
        assert_eq!(params["prompt"], "a poster");
        assert_eq!(params["format"], "a3");
        assert_eq!(params["template"], "hero");
        assert_eq!(params["style"], json!(["warm", "bold"]));
        // An unset choice is left out, not sent as an empty string the engine would refuse.
        app.ai_rt.format = String::new();
        app.ai_rt.template = String::new();
        app.ai_rt.mood.clear();
        let params = app.ai_rt.create_params();
        assert!(params.get("format").is_none() && params.get("template").is_none() && params.get("style").is_none());
        // Build without an answer is a message, not a command with null parameters.
        run(&mut app, Action::Build);
        assert!(app.ai_rt.message.as_deref().is_some_and(|message| message.contains("first")));
        assert!(app.session.documents().is_empty());
    }
}
