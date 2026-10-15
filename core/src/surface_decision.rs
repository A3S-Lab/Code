//! Turn structured tool output into Studio Intelligent UI.
//!
//! Any successful tool result can qualify. Field types become the card:
//! strings, numbers, booleans, sliders and choices become a form. A chart or
//! a list of records stays a view. `state.form` declares an editable card and
//! `state.view` declares a view. The host paints a declaration directly. Other
//! documents still ask the gateway. The chat model does not choose the surface.

use serde_json::{Map, Number, Value};
use std::collections::{HashMap, HashSet};

const LEXICAL_MODEL_PREFIX: &str = "apofasi-lexical-";
/// A three-way tie sits near zero. A usable inferred surface is peaked.
const CONFIDENCE_FLOOR: f64 = 0.5;
const STATE_CAP: usize = 4_000;
const FIELD_CAP: usize = 12;
const LABEL_CAP: usize = 80;
const TEXT_CAP: usize = 500;
const NODE_CAP: u32 = 24;
const SCAN_CAP: usize = 65_536;
const PARSE_CAP: usize = 32;
const PARSE_ATTEMPT_CAP: usize = 256;
const RECORD_CAP: usize = 4;

const PROTOCOL_KEYS: &[&str] = &[
    "schema",
    "ok",
    "success",
    "error",
    "status",
    "exit_code",
    "code",
    "permission_grant",
    "wrote_files",
    "parked_on_log",
    "question_id",
    "tool_call_id",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Surface {
    Form,
    View,
}

pub(crate) struct FormSurface {
    pub(crate) openui: String,
    pub(crate) prompt: String,
    pub(crate) model_note: String,
    /// A form waits for submitted values. A view is painted into the transcript.
    pub(crate) interactive: bool,
}

struct DisplayDoc {
    title: String,
    body: Value,
    /// The tool put the fields under `state.form`. That declares an editable card.
    declared_form: bool,
    /// The tool put the fields under `state.view`. That declares a view.
    declared_view: bool,
    /// Display names supplied by the tool. Absent keys stay machine keys.
    labels: HashMap<String, String>,
}

struct Sink {
    lines: Vec<String>,
    seq: u32,
    labels: HashMap<String, String>,
    names: HashSet<String>,
    /// A form field key cannot be submitted under a generated name.
    unnamed: bool,
}

#[cfg(test)]
pub(crate) fn display_document(output: &str) -> Option<Value> {
    let doc = find_document(output)?;
    Some(serde_json::json!({ "title": doc.title, "body": doc.body, "labels": doc.labels }))
}

pub(crate) fn decision_body(document: &Value) -> Value {
    let state = serde_json::to_string(document).unwrap_or_default();
    let state = truncate_chars(&state, STATE_CAP);
    serde_json::json!({
        "state": state,
        "questions": {
            "surface": {
                "type": "choice",
                "instructions": "How should this structured data be shown to the user?",
                "criteria": {
                    "form": "The user should fill in, edit, or confirm values. Use an interactive form.",
                    "view": "The user should see the data as an interface: text, metrics, or a chart. No input is required.",
                    "prose": "The data should stay ordinary text."
                }
            }
        }
    })
}

/// Lexical `form` or `view` only. `prose`, a non-lexical model, and a bad
/// body are absent.
#[cfg(test)]
pub(crate) fn lexical_surface(bytes: &[u8]) -> Option<Surface> {
    let value = serde_json::from_slice::<Value>(bytes).ok()?;
    let model = value.get("model").and_then(|item| item.as_str())?;
    if !model.starts_with(LEXICAL_MODEL_PREFIX) {
        return None;
    }
    match value
        .pointer("/answers/surface/choice")
        .and_then(|choice| choice.as_str())
    {
        Some("form") => Some(Surface::Form),
        Some("view") => Some(Surface::View),
        _ => None,
    }
}

pub(crate) fn openui_for(document: &Value, surface: Surface) -> Option<String> {
    let title = document
        .get("title")
        .and_then(|title| title.as_str())
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .unwrap_or("详情");
    let body = document.get("body")?;
    let mut sink = Sink {
        lines: Vec::new(),
        seq: 0,
        labels: string_labels(document.get("labels")),
        names: HashSet::new(),
        unnamed: false,
    };
    let children = project_body(&mut sink, surface, body, 0)?;
    if children.is_empty() || sink.seq > NODE_CAP {
        return None;
    }
    let mut children = children;
    if surface == Surface::Form {
        if sink.unnamed || !sink.lines.iter().any(|line| line_is_control(line)) {
            return None;
        }
        sink.lines
            .push("submit = Button(\"submit\", \"提交\")".to_owned());
        children.push("submit".to_owned());
    }
    let title = truncate_chars(title, LABEL_CAP);
    sink.lines.push(format!(
        "root = Card({}, [{}])",
        quote(&title),
        children.join(", ")
    ));
    let source = sink.lines.join("\n");
    let roots = source
        .lines()
        .filter(|line| line.trim_start().starts_with("root = "))
        .count();
    if roots == 1 {
        Some(source)
    } else {
        None
    }
}

pub(crate) const DECLARED_SURFACE_REFUSAL: &str =
    "The tool declared a surface the host could not project. Do not draw another card.";

pub(crate) struct SurfaceTrace {
    pub(crate) args: Value,
    pub(crate) output: String,
    pub(crate) exit_code: i32,
    pub(crate) form: Option<FormSurface>,
    /// Replaces the tool JSON in the model context when the host refuses a declaration.
    pub(crate) model_note: Option<String>,
}

/// Decide a surface for structured tool output.
///
/// Returns `None` when the output is not a display document. A document always
/// returns a trace, including a missing credential or a refused reply.
pub(crate) async fn decide_tool_surface(
    output: &str,
    session: Option<crate::llm::GatewayDecisionAuth>,
) -> Option<SurfaceTrace> {
    if !output.contains('{') && !output.contains('[') {
        return None;
    }
    if let Some(trace) = declared_trace(output) {
        return Some(trace);
    }
    let found = find_document(output)?;
    let document = document_value(&found);
    let form_ready = openui_for(&document, Surface::Form).is_some();
    let view_ready = openui_for(&document, Surface::View).is_some();
    if !form_ready && !view_ready {
        return None;
    }
    let body = decision_body(&document);
    let Some((url, key)) = resolve_auth(session) else {
        return Some(SurfaceTrace {
            args: body,
            output: "unavailable: no gateway credential".to_owned(),
            exit_code: 1,
            form: None,
            model_note: None,
        });
    };
    let bytes = match post_system_one(&url, &key, &body).await {
        Ok(bytes) => bytes,
        Err(message) => {
            return Some(SurfaceTrace {
                args: body,
                output: message,
                exit_code: 1,
                form: None,
                model_note: None,
            });
        }
    };
    let (surface, output_text) = explain_reply(&bytes);
    let Some(surface) = surface else {
        let exit_code = i32::from(!output_text.starts_with("choice="));
        return Some(SurfaceTrace {
            args: body,
            output: output_text,
            exit_code,
            form: None,
            model_note: None,
        });
    };
    let Some(form) = painted(&document, surface) else {
        return Some(SurfaceTrace {
            args: body,
            output: format!("{output_text}\nunavailable: OpenUI projection failed"),
            exit_code: 1,
            form: None,
            model_note: None,
        });
    };
    Some(SurfaceTrace {
        args: body,
        output: output_text,
        exit_code: 0,
        form: Some(form),
        model_note: None,
    })
}

const DECLARATION_CAP: usize = 8;

fn document_value(found: &DisplayDoc) -> Value {
    serde_json::json!({ "title": found.title, "body": found.body, "labels": found.labels })
}

/// Keep projectable forms, projectable views, and refusals apart.
/// A form is the surface when any form projects. Richness ranks only inside
/// that group, so eight richer views cannot evict a smaller form. When nothing
/// projects, refuse. Each JSON value is parsed once. Braces inside it do not
/// spend another attempt, so a later declaration is still reached.
fn declared_trace(output: &str) -> Option<SurfaceTrace> {
    let slice = scan_prefix(output);
    let mut forms: Vec<(u32, SurfaceTrace)> = Vec::new();
    let mut views: Vec<(u32, SurfaceTrace)> = Vec::new();
    let mut refusal: Option<(u32, SurfaceTrace)> = None;
    let mut parses = 0;
    let mut cursor = 0usize;
    while cursor < slice.len() {
        let Some(rel) = slice[cursor..].find(['{', '[']) else {
            break;
        };
        cursor += rel;
        if parses >= PARSE_ATTEMPT_CAP {
            break;
        }
        parses += 1;
        let Some((value, consumed)) = next_json_span(&slice[cursor..]) else {
            cursor += 1;
            continue;
        };
        cursor += consumed;
        let Some(found) = normalize(value) else {
            continue;
        };
        if !(found.declared_form || found.declared_view) {
            continue;
        }
        let score = document_score(&found);
        let document = document_value(&found);
        let surface = if found.declared_form {
            Surface::Form
        } else {
            Surface::View
        };
        let label = if found.declared_form {
            "declared=form"
        } else {
            "declared=view"
        };
        if let Some(form) = painted(&document, surface) {
            let group = if found.declared_form {
                &mut forms
            } else {
                &mut views
            };
            group.push((
                score,
                SurfaceTrace {
                    args: decision_body(&document),
                    output: label.to_owned(),
                    exit_code: 0,
                    form: Some(form),
                    model_note: None,
                },
            ));
            group.sort_by_key(|entry| std::cmp::Reverse(entry.0));
            group.truncate(DECLARATION_CAP);
            continue;
        }
        let replace = refusal.as_ref().is_none_or(|(current, _)| score > *current);
        if replace {
            refusal = Some((
                score,
                SurfaceTrace {
                    args: decision_body(&document),
                    output: "unavailable: declared surface could not be projected".to_owned(),
                    exit_code: 1,
                    form: None,
                    model_note: Some(DECLARED_SURFACE_REFUSAL.to_owned()),
                },
            ));
        }
    }
    forms
        .into_iter()
        .next()
        .map(|(_, trace)| trace)
        .or_else(|| views.into_iter().next().map(|(_, trace)| trace))
        .or_else(|| refusal.map(|(_, trace)| trace))
}

fn painted(document: &Value, surface: Surface) -> Option<FormSurface> {
    let openui = openui_for(document, surface)?;
    let prompt = document
        .get("title")
        .and_then(|title| title.as_str())
        .map(|title| truncate_chars(title.trim(), LABEL_CAP))
        .filter(|title| !title.is_empty())
        .unwrap_or_else(|| match surface {
            Surface::Form => "请确认并提交".to_owned(),
            Surface::View => "请查看".to_owned(),
        });
    Some(FormSurface {
        openui,
        prompt,
        model_note: "Structured data is on screen as Intelligent UI. Do not repeat its JSON."
            .to_owned(),
        interactive: surface == Surface::Form,
    })
}

pub(crate) fn systemone_url(base: &str) -> String {
    let base = base.trim().trim_end_matches('/');
    if let Some(origin) = base.strip_suffix("/v1") {
        format!("{origin}/v1/systemone")
    } else {
        format!("{base}/v1/systemone")
    }
}

fn explain_reply(bytes: &[u8]) -> (Option<Surface>, String) {
    let Ok(value) = serde_json::from_slice::<Value>(bytes) else {
        return (None, "refused: response was not JSON".to_owned());
    };
    let model = value
        .get("model")
        .and_then(|item| item.as_str())
        .unwrap_or("");
    if !model.starts_with(LEXICAL_MODEL_PREFIX) {
        return (
            None,
            format!("refused: model `{model}` is not a lexical decision model"),
        );
    }
    match value
        .pointer("/answers/surface/choice")
        .and_then(|choice| choice.as_str())
    {
        Some("form") => accept_inferred(&value, Surface::Form, "form", model),
        Some("view") => accept_inferred(&value, Surface::View, "view", model),
        Some("prose") => (None, format!("choice=prose\nmodel={model}")),
        Some(choice) => (None, format!("refused: choice `{choice}`")),
        None => (None, "refused: missing surface choice".to_owned()),
    }
}

fn accept_inferred(
    value: &Value,
    surface: Surface,
    choice: &str,
    model: &str,
) -> (Option<Surface>, String) {
    if let Some(confidence) = value
        .pointer("/answers/surface/confidence")
        .and_then(|item| item.as_f64())
    {
        if !confidence.is_finite() || confidence < CONFIDENCE_FLOOR {
            return (
                None,
                format!(
                    "abstain: confidence {confidence} is below {CONFIDENCE_FLOOR}\nmodel={model}"
                ),
            );
        }
    }
    (Some(surface), format!("choice={choice}\nmodel={model}"))
}

fn resolve_auth(session: Option<crate::llm::GatewayDecisionAuth>) -> Option<(String, String)> {
    if let Some(session) = session {
        // A session that arrived without a usable gateway credential does not
        // borrow the process key. `None` is the path that may use the environment.
        let key = session.api_key.trim();
        if key.is_empty()
            || !(session.base_url.starts_with("http://")
                || session.base_url.starts_with("https://"))
        {
            return None;
        }
        return Some((systemone_url(&session.base_url), key.to_owned()));
    }
    let key = gateway_key()?;
    Some((systemone_url(&gateway_origin()), key))
}

/// The scan window is a byte cap. Cut it on a character boundary so a
/// multibyte scalar at the cap does not panic on the tool-result path.
fn scan_prefix(text: &str) -> &str {
    let end = text.floor_char_boundary(text.len().min(SCAN_CAP));
    &text[..end]
}

fn find_document(output: &str) -> Option<DisplayDoc> {
    let slice = scan_prefix(output);
    let (_, _, value) = scan_documents(slice)?;
    normalize(value)
}

/// Rank at most `PARSE_CAP` display documents. Values that are not documents
/// do not use that budget. After it is full, a typed declaration can still
/// replace the winner. Each JSON value is parsed once. Parsing stops at
/// `PARSE_ATTEMPT_CAP` values.
fn scan_documents(slice: &str) -> Option<(bool, u32, Value)> {
    let mut best: Option<(bool, u32, Value)> = None;
    let mut ranked = 0;
    let mut parses = 0;
    let mut cursor = 0usize;
    while cursor < slice.len() {
        let Some(rel) = slice[cursor..].find(['{', '[']) else {
            break;
        };
        cursor += rel;
        if parses >= PARSE_ATTEMPT_CAP {
            break;
        }
        let declared_best = best.as_ref().is_some_and(|(declared, _, _)| *declared);
        if ranked >= PARSE_CAP && declared_best {
            break;
        }
        parses += 1;
        let Some((value, consumed)) = next_json_span(&slice[cursor..]) else {
            cursor += 1;
            continue;
        };
        cursor += consumed;
        let Some(doc) = normalize(value.clone()) else {
            continue;
        };
        let declared = doc.declared_form || doc.declared_view;
        if ranked >= PARSE_CAP && !declared {
            continue;
        }
        ranked += 1;
        let score = document_score(&doc);
        let replace = match &best {
            None => true,
            Some((current_declared, current_score, _)) => {
                outranks(declared, score, *current_declared, *current_score)
            }
        };
        if replace {
            best = Some((declared, score, value));
        }
    }
    best
}

/// Parse one object or array and return how many bytes it occupied.
/// The caller skips that span, so nested braces are not parsed again.
fn next_json_span(slice: &str) -> Option<(Value, usize)> {
    let mut values = serde_json::Deserializer::from_str(slice).into_iter::<Value>();
    let value = values.next()?.ok()?;
    if !(value.is_object() || value.is_array()) {
        return None;
    }
    let consumed = values.byte_offset();
    if consumed == 0 || !slice.is_char_boundary(consumed) {
        return None;
    }
    Some((value, consumed))
}

fn normalize(value: Value) -> Option<DisplayDoc> {
    let value = unwrap_envelope(value);
    if let Some(object) = value.as_object() {
        if let Some(root) = object.get("root").and_then(|root| root.as_str()) {
            if !root.trim().is_empty() {
                if let Some(state) = object.get("state") {
                    let declared_form = nonempty_object(state.get("form"));
                    let declared_view = !declared_form && nonempty_object(state.get("view"));
                    if declared_form || declared_view {
                        let body = if declared_form {
                            state.get("form").cloned()
                        } else {
                            state.get("view").cloned()
                        };
                        if let Some(body) = body {
                            return Some(DisplayDoc {
                                title: card_title(object, root),
                                body,
                                declared_form,
                                declared_view,
                                labels: state_labels(state),
                            });
                        }
                    }
                    if is_display(state) {
                        return Some(DisplayDoc {
                            title: card_title(object, root),
                            body: state.clone(),
                            declared_form: false,
                            declared_view: false,
                            labels: state_labels(state),
                        });
                    }
                }
            }
        }
    }
    if !is_display(&value) {
        return None;
    }
    let title = title_of(&value).unwrap_or_else(|| "详情".to_owned());
    Some(DisplayDoc {
        title,
        body: value,
        declared_form: false,
        declared_view: false,
        labels: HashMap::new(),
    })
}

fn is_declaration(value: &Value) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    let Some(root) = object.get("root").and_then(|root| root.as_str()) else {
        return false;
    };
    if root.trim().is_empty() {
        return false;
    }
    let Some(state) = object.get("state") else {
        return false;
    };
    nonempty_object(state.get("form")) || nonempty_object(state.get("view"))
}

fn nonempty_object(value: Option<&Value>) -> bool {
    value.is_some_and(|item| item.as_object().is_some_and(|fields| !fields.is_empty()))
}

fn card_title(object: &Map<String, Value>, root: &str) -> String {
    let title = object
        .get("title")
        .and_then(|title| title.as_str())
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .unwrap_or(root.trim());
    truncate_chars(title, LABEL_CAP)
}

fn state_labels(state: &Value) -> HashMap<String, String> {
    string_labels(state.get("labels"))
}

fn string_labels(value: Option<&Value>) -> HashMap<String, String> {
    let mut labels = HashMap::new();
    let Some(object) = value.and_then(|value| value.as_object()) else {
        return labels;
    };
    for (key, label) in object {
        if let Some(text) = label.as_str() {
            let text = text.trim();
            if !text.is_empty() {
                labels.insert(key.clone(), truncate_chars(text, LABEL_CAP));
            }
        }
    }
    labels
}

const ENVELOPE_KEYS: &[&str] = &["answer", "data", "result", "payload"];

fn unwrap_envelope(value: Value) -> Value {
    let Some(object) = value.as_object() else {
        return value;
    };
    if object.contains_key("root") {
        return value;
    }
    if let Some(found) = declaration_in_envelope(object) {
        return found;
    }
    for key in ENVELOPE_KEYS {
        let Some(inner) = object.get(*key) else {
            continue;
        };
        if let Some(text) = inner.as_str() {
            if let Some(parsed) = json_in_text(text) {
                return parsed;
            }
        } else if (inner.is_object() || inner.is_array()) && object.len() == 1 {
            return inner.clone();
        }
    }
    value
}

/// A form declaration in any envelope field wins. A view declaration is kept
/// when no form is present. An earlier ordinary document does not hide either.
fn declaration_in_envelope(object: &Map<String, Value>) -> Option<Value> {
    let mut view = None;
    for key in ENVELOPE_KEYS {
        let Some(inner) = object.get(*key) else {
            continue;
        };
        let Some(candidate) = envelope_candidate(inner) else {
            continue;
        };
        if !is_declaration(&candidate) {
            continue;
        }
        if form_declaration(&candidate) {
            return Some(candidate);
        }
        if view.is_none() {
            view = Some(candidate);
        }
    }
    view
}

fn envelope_candidate(inner: &Value) -> Option<Value> {
    if let Some(text) = inner.as_str() {
        return json_in_text(text);
    }
    if inner.is_object() || inner.is_array() {
        return Some(inner.clone());
    }
    None
}

fn form_declaration(value: &Value) -> bool {
    value
        .as_object()
        .and_then(|object| object.get("state"))
        .is_some_and(|state| nonempty_object(state.get("form")))
}

/// A tool often wraps a card in prose. The card is the document when the
/// whole string is JSON, or when a JSON object sits inside that prose.
fn json_in_text(text: &str) -> Option<Value> {
    let trimmed = text.trim();
    if let Ok(parsed) = serde_json::from_str::<Value>(trimmed) {
        if parsed.is_object() || parsed.is_array() {
            return Some(parsed);
        }
    }
    let slice = scan_prefix(text);
    scan_documents(slice).map(|(_, _, value)| value)
}

fn is_display(value: &Value) -> bool {
    let value = strip_protocol(value);
    has_chart(&value)
        || has_record_list(&value)
        || number_count(&value) >= 2
        || (has_human(&value) && scalar_count(&value) >= 2)
}

fn strip_protocol(value: &Value) -> Value {
    let Some(object) = value.as_object() else {
        return value.clone();
    };
    let mut kept = serde_json::Map::new();
    for (key, child) in object {
        if PROTOCOL_KEYS.contains(&key.as_str()) {
            continue;
        }
        kept.insert(key.clone(), child.clone());
    }
    Value::Object(kept)
}

fn title_of(value: &Value) -> Option<String> {
    let object = value.as_object()?;
    for key in ["title", "name", "label"] {
        if let Some(text) = object.get(key).and_then(|item| item.as_str()) {
            let text = text.trim();
            if !text.is_empty() {
                return Some(truncate_chars(text, LABEL_CAP));
            }
        }
    }
    None
}

fn project_body(
    sink: &mut Sink,
    surface: Surface,
    value: &Value,
    depth: u32,
) -> Option<Vec<String>> {
    if sink.seq >= NODE_CAP {
        return None;
    }
    if let Some(items) = value.as_array() {
        if items.iter().all(|item| item.is_object()) && items.len() >= 2 {
            let mut ids = Vec::new();
            for item in items.iter().take(RECORD_CAP) {
                let title = title_of(item).unwrap_or_else(|| format!("{}", ids.len() + 1));
                let Some(children) = project_object(sink, surface, item, depth + 1) else {
                    continue;
                };
                if children.is_empty() {
                    continue;
                }
                ids.push(sink.card(&title, &children));
            }
            return if ids.is_empty() { None } else { Some(ids) };
        }
    }
    if value.is_object() {
        return project_object(sink, surface, value, depth);
    }
    None
}

fn project_object(
    sink: &mut Sink,
    surface: Surface,
    value: &Value,
    depth: u32,
) -> Option<Vec<String>> {
    let object = value.as_object()?;
    let mut ids = Vec::new();
    let chart_keys = chart_keys(object);
    if let Some((labels, values)) = chart_of(object) {
        ids.push(sink.chart(&labels, &values));
    }
    // JSON object order can change when another workspace member enables
    // serde_json/preserve_order. Choose fields before applying the cap so the
    // same document keeps both its controls and their generated identities.
    let mut fields: Vec<_> = object.iter().collect();
    fields.sort_unstable_by_key(|(key, _)| *key);
    for (key, child) in fields.into_iter().take(FIELD_CAP) {
        if PROTOCOL_KEYS.contains(&key.as_str()) {
            continue;
        }
        if chart_keys.iter().any(|chart_key| chart_key == key) {
            continue;
        }
        if (key == "title" || key == "name" || key == "label") && child.is_string() {
            continue;
        }
        if let Some(id) = project_field(sink, surface, key, child, depth) {
            ids.push(id);
        }
    }
    if ids.is_empty() {
        None
    } else {
        Some(ids)
    }
}

fn project_field(
    sink: &mut Sink,
    surface: Surface,
    key: &str,
    value: &Value,
    depth: u32,
) -> Option<String> {
    if sink.seq >= NODE_CAP {
        return None;
    }
    let (label, value) = match labeled_scalar(value) {
        Some((custom, inner)) => (truncate_chars(custom, LABEL_CAP), inner),
        None => (
            truncate_chars(
                sink.labels.get(key).map(String::as_str).unwrap_or(key),
                LABEL_CAP,
            ),
            value,
        ),
    };
    if label.trim().is_empty() {
        return None;
    }
    match value {
        Value::String(text) => {
            let text = truncate_chars(text.trim(), TEXT_CAP);
            if text.is_empty() {
                return None;
            }
            Some(match surface {
                Surface::View => sink.text(&format!("{label}: {text}")),
                Surface::Form if text.contains('\n') || text.chars().count() > 80 => {
                    let name = control_name(sink, key)?;
                    let id = sink.alloc();
                    sink.lines.push(format!(
                        "{id} = Textarea({}, {}, {})",
                        quote(&name),
                        quote(&label),
                        quote(&text)
                    ));
                    id
                }
                Surface::Form => {
                    let name = control_name(sink, key)?;
                    let id = sink.alloc();
                    sink.lines.push(format!(
                        "{id} = Input({}, {}, {}, \"text\")",
                        quote(&name),
                        quote(&label),
                        quote(&text)
                    ));
                    id
                }
            })
        }
        Value::Number(number) => {
            let number = format_number(number)?;
            Some(match surface {
                Surface::View => {
                    let id = sink.alloc();
                    sink.lines
                        .push(format!("{id} = Metric({}, {number}, \"\")", quote(&label)));
                    id
                }
                Surface::Form => {
                    let name = control_name(sink, key)?;
                    let id = sink.alloc();
                    sink.lines.push(format!(
                        "{id} = Input({}, {}, {number}, \"number\")",
                        quote(&name),
                        quote(&label)
                    ));
                    id
                }
            })
        }
        Value::Bool(flag) => Some(match surface {
            Surface::View => sink.text(&format!("{label}: {flag}")),
            Surface::Form => {
                let name = control_name(sink, key)?;
                let id = sink.alloc();
                sink.lines.push(format!(
                    "{id} = Checkbox({}, {}, {flag})",
                    quote(&name),
                    quote(&label)
                ));
                id
            }
        }),
        Value::Object(_) if depth < 2 => {
            if let Some(id) = slider_field(sink, surface, key, &label, value) {
                return Some(id);
            }
            if let Some(id) = select_field(sink, surface, key, &label, value) {
                return Some(id);
            }
            let children = project_object(sink, surface, value, depth + 1)?;
            if children.is_empty() {
                return None;
            }
            Some(sink.card(&label, &children))
        }
        Value::Array(items)
            if depth < 2 && items.iter().all(|item| item.is_object()) && items.len() >= 2 =>
        {
            let mut ids = Vec::new();
            for item in items.iter().take(RECORD_CAP) {
                let title = title_of(item).unwrap_or_else(|| format!("{}", ids.len() + 1));
                let Some(children) = project_object(sink, surface, item, depth + 1) else {
                    continue;
                };
                if children.is_empty() {
                    continue;
                }
                ids.push(sink.card(&title, &children));
            }
            if ids.is_empty() {
                None
            } else {
                Some(sink.stack(&label, &ids))
            }
        }
        _ => None,
    }
}

fn labeled_scalar(value: &Value) -> Option<(&str, &Value)> {
    let object = value.as_object()?;
    if object.contains_key("options") || object.contains_key("min") || object.contains_key("max") {
        return None;
    }
    let label = object.get("label")?.as_str()?.trim();
    if label.is_empty() {
        return None;
    }
    let inner = object.get("value")?;
    if inner.is_string() || inner.is_number() || inner.is_boolean() {
        Some((label, inner))
    } else {
        None
    }
}

fn control_name(sink: &mut Sink, key: &str) -> Option<String> {
    if is_control_name(key) && sink.names.insert(key.to_owned()) {
        return Some(key.to_owned());
    }
    sink.unnamed = true;
    None
}

fn is_control_name(key: &str) -> bool {
    let mut chars = key.chars();
    match chars.next() {
        Some(first) if first.is_ascii_alphabetic() => {}
        _ => return false,
    }
    if key.chars().count() > 48 {
        return false;
    }
    if !key
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
    {
        return false;
    }
    !matches!(
        key,
        "root" | "submit" | "__proto__" | "prototype" | "constructor"
    )
}

fn slider_field(
    sink: &mut Sink,
    surface: Surface,
    key: &str,
    label: &str,
    value: &Value,
) -> Option<String> {
    let object = value.as_object()?;
    let initial = object.get("value").or_else(|| object.get("initial"))?;
    let number = format_number(initial.as_number()?)?;
    let min = format_number(object.get("min")?.as_number()?)?;
    let max = format_number(object.get("max")?.as_number()?)?;
    let initial_f = initial.as_number()?.as_f64()?;
    let min_f = object.get("min")?.as_f64()?;
    let max_f = object.get("max")?.as_f64()?;
    if min_f >= max_f || initial_f < min_f || initial_f > max_f {
        return None;
    }
    let step = object
        .get("step")
        .and_then(|step| step.as_number())
        .and_then(format_number);
    let id = sink.alloc();
    if surface == Surface::Form {
        let step_arg = step
            .as_deref()
            .map(|step| format!(", {step}"))
            .unwrap_or_default();
        let name = control_name(sink, key)?;
        sink.lines.push(format!(
            "{id} = Slider({}, {}, {number}, {min}, {max}{step_arg})",
            quote(&name),
            quote(label)
        ));
    } else {
        sink.lines
            .push(format!("{id} = Metric({}, {number}, \"\")", quote(label)));
    }
    Some(id)
}

fn select_field(
    sink: &mut Sink,
    surface: Surface,
    key: &str,
    label: &str,
    value: &Value,
) -> Option<String> {
    let object = value.as_object()?;
    let selected = object
        .get("value")
        .or_else(|| object.get("initial"))
        .and_then(|item| item.as_str())
        .map(str::trim)?;
    let options = object.get("options")?.as_array()?;
    if options.is_empty() || options.len() > 8 {
        return None;
    }
    let mut texts = Vec::new();
    for option in options {
        let text = option.as_str()?.trim();
        if text.is_empty() || texts.iter().any(|existing: &String| existing == text) {
            return None;
        }
        texts.push(truncate_chars(text, LABEL_CAP));
    }
    if !texts.iter().any(|option| option == selected) {
        return None;
    }
    if surface == Surface::View {
        return Some(sink.text(&format!("{label}: {selected}")));
    }
    let id = sink.alloc();
    let rendered = texts
        .iter()
        .map(|option| quote(option))
        .collect::<Vec<_>>()
        .join(", ");
    let name = control_name(sink, key)?;
    sink.lines.push(format!(
        "{id} = Select({}, {}, {}, [{rendered}])",
        quote(&name),
        quote(label),
        quote(selected)
    ));
    Some(id)
}

fn chart_keys(object: &serde_json::Map<String, Value>) -> Vec<String> {
    chart_pair(object)
        .map(|(left, right, _, _)| vec![left, right])
        .unwrap_or_default()
}

fn chart_of(object: &serde_json::Map<String, Value>) -> Option<(Vec<String>, Vec<String>)> {
    let (_, _, labels, values) = chart_pair(object)?;
    Some((labels, values))
}

fn chart_pair(
    object: &serde_json::Map<String, Value>,
) -> Option<(String, String, Vec<String>, Vec<String>)> {
    let mut labels: Option<(String, Vec<String>)> = None;
    let mut values: Option<(String, Vec<String>)> = None;
    for (key, value) in object {
        let Some(items) = value.as_array() else {
            continue;
        };
        if items.len() < 2 || items.len() > 24 {
            continue;
        }
        if items.iter().all(|item| item.as_str().is_some()) && labels.is_none() {
            let texts = items
                .iter()
                .filter_map(|item| item.as_str())
                .map(|text| truncate_chars(text.trim(), LABEL_CAP))
                .collect::<Vec<_>>();
            if texts.iter().any(|text| text.is_empty()) {
                continue;
            }
            labels = Some((key.clone(), texts));
        } else if items.iter().all(|item| item.as_number().is_some()) && values.is_none() {
            let mut numbers = Vec::new();
            let mut ok = true;
            for item in items {
                let Some(number) = item.as_f64() else {
                    ok = false;
                    break;
                };
                if !number.is_finite() || number < 0.0 {
                    ok = false;
                    break;
                }
                let Some(text) = item.as_number().and_then(format_number) else {
                    ok = false;
                    break;
                };
                numbers.push(text);
            }
            if ok {
                values = Some((key.clone(), numbers));
            }
        }
    }
    let (label_key, labels) = labels?;
    let (value_key, values) = values?;
    if labels.len() != values.len() || label_key == value_key {
        return None;
    }
    Some((label_key, value_key, labels, values))
}

fn has_chart(value: &Value) -> bool {
    value.as_object().and_then(chart_of).is_some()
        || value
            .as_array()
            .is_some_and(|items| items.iter().any(has_chart))
}

fn has_record_list(value: &Value) -> bool {
    let Some(items) = value.as_array() else {
        return value
            .as_object()
            .is_some_and(|object| object.values().any(has_record_list));
    };
    items.len() >= 2
        && items.iter().all(|item| {
            item.as_object()
                .is_some_and(|object| object.values().map(scalar_count).sum::<u32>() >= 1)
        })
}

fn has_human(value: &Value) -> bool {
    match value {
        Value::String(text) => human_text(text),
        Value::Array(items) => items.iter().any(has_human),
        Value::Object(object) => object.values().any(has_human),
        _ => false,
    }
}

fn human_text(text: &str) -> bool {
    let text = text.trim();
    !text.is_empty() && (!text.is_ascii() || text.contains(' ') || text.chars().count() >= 12)
}

fn number_count(value: &Value) -> u32 {
    match value {
        Value::Number(_) => 1,
        Value::Array(items) => items.iter().map(number_count).sum(),
        Value::Object(object) => object.values().map(number_count).sum(),
        _ => 0,
    }
}

fn scalar_count(value: &Value) -> u32 {
    match value {
        Value::String(text) if !text.trim().is_empty() => 1,
        Value::Number(_) | Value::Bool(_) => 1,
        Value::Array(items) => items.iter().map(scalar_count).sum(),
        Value::Object(object) => object.values().map(scalar_count).sum(),
        _ => 0,
    }
}

/// A typed declaration outranks every undeclared document. Richness only
/// ranks documents in the same class.
fn outranks(declared: bool, score: u32, best_declared: bool, best_score: u32) -> bool {
    (declared && !best_declared) || (declared == best_declared && score > best_score)
}

fn document_score(doc: &DisplayDoc) -> u32 {
    weight(&doc.body)
}

fn weight(value: &Value) -> u32 {
    scalar_count(value) + u32::from(has_chart(value)) * 4 + u32::from(has_record_list(value)) * 4
}

impl Sink {
    fn alloc(&mut self) -> String {
        let id = format!("n{}", self.seq);
        self.seq += 1;
        id
    }

    fn text(&mut self, text: &str) -> String {
        let id = self.alloc();
        self.lines.push(format!(
            "{id} = Text({})",
            quote(&truncate_chars(text, TEXT_CAP))
        ));
        id
    }

    fn card(&mut self, title: &str, children: &[String]) -> String {
        let id = self.alloc();
        self.lines.push(format!(
            "{id} = Card({}, [{}])",
            quote(&truncate_chars(title, LABEL_CAP)),
            children.join(", ")
        ));
        id
    }

    fn stack(&mut self, title: &str, children: &[String]) -> String {
        let id = self.alloc();
        self.lines.push(format!(
            "{id} = Card({}, [{}])",
            quote(&truncate_chars(title, LABEL_CAP)),
            children.join(", ")
        ));
        id
    }

    fn chart(&mut self, labels: &[String], values: &[String]) -> String {
        let id = self.alloc();
        let labels = labels
            .iter()
            .map(|label| quote(label))
            .collect::<Vec<_>>()
            .join(", ");
        self.lines.push(format!(
            "{id} = BarChart([{labels}], [{}])",
            values.join(", ")
        ));
        id
    }
}

fn format_number(number: &Number) -> Option<String> {
    if let Some(value) = number.as_i64() {
        return Some(value.to_string());
    }
    if let Some(value) = number.as_u64() {
        return Some(value.to_string());
    }
    let value = number.as_f64()?;
    if !value.is_finite() {
        return None;
    }
    let text = format!("{value:.4}");
    let text = text.trim_end_matches('0').trim_end_matches('.').to_owned();
    if text.is_empty() || text.len() > 24 {
        return None;
    }
    Some(text)
}

fn line_is_control(line: &str) -> bool {
    line.contains(" = Input(")
        || line.contains(" = Textarea(")
        || line.contains(" = Checkbox(")
        || line.contains(" = Select(")
        || line.contains(" = Slider(")
}

fn quote(value: &str) -> String {
    let mut out = String::from("\"");
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' | '\r' => out.push(' '),
            ch if ch.is_control() => {}
            ch => out.push(ch),
        }
    }
    out.push('"');
    out
}

fn truncate_chars(value: &str, max: usize) -> String {
    value.chars().take(max).collect()
}

fn gateway_origin() -> String {
    for name in ["A3S_GATEWAY_ORIGIN", "A3S_GATEWAY_BASE_URL"] {
        if let Ok(value) = std::env::var(name) {
            let value = value.trim().trim_end_matches('/').to_owned();
            if value.starts_with("http://") || value.starts_with("https://") {
                return value;
            }
        }
    }
    "http://127.0.0.1:8080".to_owned()
}

fn gateway_key() -> Option<String> {
    let key = std::env::var("A3S_GATEWAY_KEY").ok()?;
    let key = key.trim().to_owned();
    if key.is_empty() {
        None
    } else {
        Some(key)
    }
}

async fn post_system_one(url: &str, key: &str, body: &Value) -> Result<Vec<u8>, String> {
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(2))
        .timeout(std::time::Duration::from_secs(8))
        .build()
        .map_err(|_| "unavailable: could not open a gateway client".to_owned())?;
    let response = match client.post(url).bearer_auth(key).json(body).send().await {
        Ok(response) => response,
        Err(_) => return Err("unavailable: gateway timed out".to_owned()),
    };
    let status = response.status();
    if !status.is_success() {
        return Err(format!("unavailable: HTTP {}", status.as_u16()));
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|_| "unavailable: empty gateway response".to_owned())?;
    if bytes.len() > 65_536 {
        return Err("unavailable: gateway response is too large".to_owned());
    }
    Ok(bytes.to_vec())
}

#[cfg(test)]
#[path = "surface_decision_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "surface_decision_e2e.rs"]
mod e2e;
