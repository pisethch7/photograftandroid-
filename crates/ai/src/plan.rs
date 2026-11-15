//! The plan: a design becomes the engine commands that build it.
//!
//! Every element is one of the ordinary commands a person could have run by hand — `file.new`,
//! `layer.newFillLayer.*`, `shape.create`, `type.create` — so the result is an ordinary document:
//! real type layers that can be retyped, real shape layers that can be reshaped, and layers that
//! rename, hide, group and take a plug-in exactly like any other.
//!
//! A step whose command answers with the layer it made can be named by the step after it, through
//! a [`Ref`]: `layer.newFillLayer.*` and `shape.create` have no `name` parameter of their own, so
//! the plan follows them with `layer.renameLayer` pointing at the id the earlier step returned.

use serde_json::{Value, json};

use crate::design::{Align, Design, Element, Paint, Rect, ShapeElement, TextElement};

/// The name the whole build records in the History panel.
pub const HISTORY_LABEL: &str = "AI Poster";

/// One command to run, and its parameters.
#[derive(Clone, Debug, PartialEq)]
pub struct Step {
    pub command: &'static str,
    pub params: Value,
}

impl Step {
    fn new(command: &'static str, params: Value) -> Self {
        Self { command, params }
    }

    /// The step as JSON, for the UI's preview and for logs. Never contains a secret: a step is
    /// words, colours and geometry, and nothing else.
    pub fn to_json(&self) -> Value {
        json!({ "command": self.command, "params": self.params })
    }
}

/// A reference to the layer an earlier step returned, written into a later step's parameters as
/// `{"$layer": <step index>}`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ref(pub usize);

impl Ref {
    /// The reference, as it appears in a step's parameters.
    pub fn marker(self) -> Value {
        json!({ "$layer": self.0 })
    }
}

/// Build the document named `name` from `design`: the steps, in the order they must run.
pub fn steps(design: &Design, name: &str) -> Vec<Step> {
    let mut plan = Vec::with_capacity(design.elements.len() + 3);
    plan.push(Step::new(
        "file.new",
        json!({
            "width": design.format.width,
            "height": design.format.height,
            "resolution": round1(design.format.dpi),
            "background": design.paper.hex(),
            "mode": "rgb",
            "depth": 8,
            "name": name,
        }),
    ));
    for element in &design.elements {
        match element {
            Element::Fill { name: layer_name, paint } => fill(&mut plan, layer_name, *paint),
            Element::Shape(shape) => plan.push(Step::new("shape.create", shape_params(shape))),
            Element::Text(text) => plan.push(Step::new("type.create", text_params(text, design))),
        }
    }
    plan
}

/// A fill layer, then the rename that gives it the template's name.
fn fill(plan: &mut Vec<Step>, name: &str, paint: Paint) {
    let params = match paint {
        Paint::Solid(color) => json!({ "color": color.hex() }),
        Paint::Gradient { from, to, angle } => json!({
            "from": from.hex(),
            "to": to.hex(),
            "angle": round1(angle),
            "style": "linear",
        }),
    };
    let command = match paint {
        Paint::Solid(_) => "layer.newFillLayer.solidColor",
        Paint::Gradient { .. } => "layer.newFillLayer.gradient",
    };
    let at = plan.len();
    plan.push(Step::new(command, params));
    // The fill commands take no name of their own, so the layer is named in its own step.
    plan.push(Step::new("layer.renameLayer", json!({ "layer": Ref(at).marker(), "name": name })));
}

fn shape_params(shape: &ShapeElement) -> Value {
    let mut params = json!({
        "kind": "rect",
        "rect": rect_json(shape.rect),
        "name": shape.name,
    });
    params["fill"] = match shape.fill {
        Some(color) => json!(color.hex()),
        None => Value::Null,
    };
    if let Some((color, width)) = shape.stroke {
        params["stroke"] = json!({ "width": round1(width.max(1.0)), "color": color.hex(), "align": "inside" });
    }
    if shape.radius > 0.5 {
        params["radii"] = json!(round1(shape.radius));
    }
    params
}

fn text_params(text: &TextElement, design: &Design) -> Value {
    let mut params = json!({
        "x": round(text.rect.x),
        "y": round(text.rect.y),
        "text": text.text,
        "box": rect_json(text.rect),
        "name": text.name,
        "align": align_name(text.align),
        "size": round2(design.format.pt(text.size_px)),
        "color": text.color.hex(),
        "weight": text.weight,
        "tracking": round1(text.tracking),
        "leading": round2(design.format.pt(text.size_px * text.leading)),
    });
    if text.caps {
        params["caps"] = json!("all");
    }
    params
}

fn align_name(align: Align) -> &'static str {
    align.name()
}

fn rect_json(rect: Rect) -> Value {
    json!([round(rect.x), round(rect.y), round(rect.w), round(rect.h)])
}

/// A whole pixel.
fn round(value: f32) -> i64 {
    if value.is_finite() { value.round() as i64 } else { 0 }
}

/// One decimal place, for angles and stroke widths.
fn round1(value: f32) -> f64 {
    if value.is_finite() { (f64::from(value) * 10.0).round() / 10.0 } else { 0.0 }
}

/// Two decimal places, for point sizes.
fn round2(value: f32) -> f64 {
    if value.is_finite() { (f64::from(value) * 100.0).round() / 100.0 } else { 0.0 }
}

/// The commands a plan may contain. A test keeps this list exact, so a typo in a step name cannot
/// reach the engine and fail there instead of here.
pub const COMMANDS: &[&str] = &["file.new", "layer.newFillLayer.solidColor", "layer.newFillLayer.gradient", "layer.renameLayer", "shape.create", "type.create"];

/// Every layer id a step returns, one entry per step; the engine's executor fills this in as it
/// runs, so a [`Ref`] can name the layer an earlier step made.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Created {
    pub layers: Vec<Option<u64>>,
}

impl Created {
    /// Record what a step returned: the layer, when the command made one. Called once per step, so
    /// the entries stay aligned with the plan's indices.
    pub fn push(&mut self, result: &Value) {
        self.layers.push(result.get("layer").and_then(Value::as_u64));
    }

    /// Replace every [`Ref`] in `params` with the layer it names, leaving the rest untouched.
    pub fn resolve(&self, params: &Value) -> Result<Value, String> {
        match params {
            Value::Object(fields) => {
                if fields.len() == 1
                    && let Some(at) = fields.get("$layer").and_then(Value::as_u64)
                {
                    let at = usize::try_from(at).map_err(|_| format!("layer reference {at} is out of range"))?;
                    let id = self.layers.get(at).copied().flatten().ok_or_else(|| format!("layer reference {at} was not made by an earlier step"))?;
                    return Ok(json!(id));
                }
                let mut out = serde_json::Map::with_capacity(fields.len());
                for (key, value) in fields {
                    out.insert(key.clone(), self.resolve(value)?);
                }
                Ok(Value::Object(out))
            }
            Value::Array(items) => {
                let mut out = Vec::with_capacity(items.len());
                for item in items {
                    out.push(self.resolve(item)?);
                }
                Ok(Value::Array(out))
            }
            other => Ok(other.clone()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brief::Brief;
    use crate::template;

    fn design(v: Value) -> Design {
        let brief = Brief::from_json(&v).expect("valid brief");
        template::compose(&brief).expect("composes")
    }

    fn params_text(step: &Step) -> String {
        step.params.to_string()
    }

    #[test]
    fn the_first_step_opens_the_canvas_at_the_designs_size() {
        let d = design(json!({"title": "Fair", "format": "a3"}));
        let plan = steps(&d, "Autumn Fair");
        let first = plan.first().expect("a first step");
        assert_eq!(first.command, "file.new");
        assert_eq!(first.params["width"], 3508);
        assert_eq!(first.params["height"], 4961);
        assert_eq!(first.params["resolution"], 300.0);
        assert_eq!(first.params["name"], "Autumn Fair");
        assert_eq!(first.params["background"], d.paper.hex());
    }

    #[test]
    fn every_step_is_a_command_we_know() {
        for preset in crate::format::PRESETS {
            for template in template::IDS {
                let d = design(json!({
                    "title": "Autumn Print Fair",
                    "subtitle": "Twelve studios",
                    "body": "Prints and posters.",
                    "footer": "Sat 14 Nov",
                    "format": preset.id,
                    "template": template,
                }));
                for step in steps(&d, "Poster") {
                    assert!(COMMANDS.contains(&step.command), "{} is not in COMMANDS", step.command);
                    assert!(step.params.is_object(), "{}", step.command);
                    assert!(!params_text(&step).contains("NaN"), "{}", step.command);
                }
            }
        }
    }

    #[test]
    fn one_step_per_element_plus_the_canvas_and_a_rename_per_fill() {
        let d = design(json!({"title": "Fair", "template": "hero", "format": "square"}));
        let plan = steps(&d, "Poster");
        let fills = d.elements.iter().filter(|e| matches!(e, Element::Fill { .. })).count();
        assert_eq!(plan.len(), 1 + d.elements.len() + fills);
        assert_eq!(plan.iter().filter(|s| s.command == "layer.renameLayer").count(), fills);
    }

    #[test]
    fn type_sizes_are_points_at_the_documents_resolution() {
        let print = design(json!({"title": "Print", "format": "a4"}));
        let step = steps(&print, "P").into_iter().find(|s| s.command == "type.create").expect("a type layer");
        let px = print.texts().next().expect("headline").size_px;
        let pt = step.params["size"].as_f64().expect("a size");
        assert!((pt - f64::from(print.format.pt(px))).abs() < 0.02, "pt {pt} for {px} px");
        assert!(pt < f64::from(px), "a 300 dpi poster sets smaller points than pixels");

        let web = design(json!({"title": "Web", "format": "square"}));
        let step = steps(&web, "P").into_iter().find(|s| s.command == "type.create").expect("a type layer");
        let px = web.texts().next().expect("headline").size_px;
        let pt = step.params["size"].as_f64().expect("a size");
        assert!((pt - f64::from(px)).abs() < 0.02, "72 dpi: {pt} should equal {px}");
    }

    #[test]
    fn a_gradient_fill_is_named_by_the_step_after_it() {
        let d = design(json!({"title": "Hero", "template": "hero", "format": "square"}));
        let plan = steps(&d, "P");
        let gradient = plan.iter().position(|s| s.command == "layer.newFillLayer.gradient").expect("a gradient");
        let rename = &plan[gradient + 1];
        assert_eq!(rename.command, "layer.renameLayer");
        assert_eq!(rename.params["layer"], json!({"$layer": gradient}));
        assert_eq!(rename.params["name"], "Background Gradient");
    }

    #[test]
    fn references_resolve_to_the_layer_created_before_them() {
        let d = design(json!({"title": "Hero", "template": "hero", "format": "square"}));
        let plan = steps(&d, "P");
        let mut created = Created::default();
        for (i, step) in plan.iter().enumerate() {
            let params = created.resolve(&step.params).expect("resolves");
            if plan[i].command == "layer.renameLayer" {
                assert!(params["layer"].as_u64().is_some(), "rename {i} did not resolve: {params}");
            }
            assert!(!params.to_string().contains("$layer"));
            // Pretend the step returned a layer id.
            created.push(&json!({ "layer": i as u64 + 100 }));
        }
        assert_eq!(created.layers.len(), plan.len());
    }

    #[test]
    fn a_dangling_reference_is_an_error_not_a_panic() {
        let created = Created::default();
        assert!(created.resolve(&json!({"$layer": 3})).is_err());
        assert!(created.resolve(&json!({"$layer": 3})).is_err());
        assert!(created.resolve(&json!([1, 2, {"$layer": 0}])).is_err());
    }

    #[test]
    fn resolving_leaves_ordinary_values_alone() {
        let mut created = Created::default();
        created.push(&json!({ "layer": 7 }));
        let params = json!({"name": "Layer", "nested": [1, 2.5, "x"], "flag": true, "nothing": null});
        assert_eq!(created.resolve(&params).expect("resolves"), params);
    }

    #[test]
    fn a_shape_carries_its_fill_stroke_and_radii() {
        let d = design(json!({"title": "Frame", "template": "frame", "format": "a4"}));
        let plan = steps(&d, "P");
        let frame = plan.iter().find(|s| s.params["name"] == "Frame").expect("the frame shape");
        assert_eq!(frame.command, "shape.create");
        assert!(frame.params["fill"].is_null());
        assert!(frame.params["stroke"]["width"].as_f64().unwrap_or(0.0) >= 1.0);
        assert!(frame.params["stroke"]["color"].as_str().is_some_and(|c| c.starts_with('#')));
    }

    #[test]
    fn a_capitalised_footer_is_set_in_capitals() {
        let d = design(json!({"title": "Fair", "footer": "Sat 14 Nov", "template": "stack", "format": "a4"}));
        let plan = steps(&d, "P");
        let footer = plan.iter().find(|s| s.params["name"] == "Details").expect("the details line");
        assert_eq!(footer.params["caps"], "all");
        assert!(footer.params["tracking"].as_f64().unwrap_or(0.0) > 0.0);
    }

    #[test]
    fn a_plan_is_deterministic_and_free_of_non_finite_numbers() {
        let d = design(json!({"title": "Repeatable", "body": "Body", "footer": "Now", "format": "a4"}));
        let a: Vec<Value> = steps(&d, "P").iter().map(Step::to_json).collect();
        let b: Vec<Value> = steps(&d, "P").iter().map(Step::to_json).collect();
        assert_eq!(a, b);
        for step in &a {
            assert!(!step.to_string().contains("null") || step["params"]["fill"] == Value::Null);
        }
    }

    #[test]
    fn the_longest_text_a_brief_allows_still_plans() {
        let body = "word ".repeat(300);
        let d = design(json!({"title": "Long", "body": body, "format": "story", "template": "stack"}));
        let plan = steps(&d, "P");
        assert!(plan.iter().any(|s| s.command == "type.create"));
        for step in &plan {
            assert!(!step.params.to_string().contains("NaN"));
        }
    }
}
