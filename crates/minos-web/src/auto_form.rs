//! JSON-schema → HTML form walker for third-party filter kinds.
//!
//! Built-in kinds (`regex`, `http`, `python_sidecar`) have hand-written
//! editor templates. Any other registered kind gets a form generated from its
//! `schemars`-derived JSON schema instead of a raw JSON textarea, so operators
//! can fill in typed fields.
//!
//! Two complementary walks over the same schema:
//! - [`render_form`] turns the schema (+ current values) into HTML inputs,
//!   each named with a dot-path under `config` (e.g. `config.threshold`).
//! - [`form_to_config`] turns the submitted dot-keyed form back into a JSON
//!   config object, coercing each value to the schema's declared type.
//!
//! Supported types: `boolean`, `integer`/`number`, `string`, nested `object`,
//! and `array` (edited as a JSON snippet). `Option<T>` fields (whose schema
//! `type` is `["T", "null"]`) are handled by taking the first non-null type.

use std::collections::HashMap;
use std::fmt::Write as _;

use serde_json::{Map, Value};

/// Top-level form-field prefix so generated inputs never collide with the
/// editor's own fields (e.g. `display_name`).
const ROOT: &str = "config";

/// Render an HTML form body for `schema`, pre-populating from `current`.
#[must_use]
pub fn render_form(schema: &Value, current: &Value) -> String {
    let mut out = String::new();
    render_object(schema, current, ROOT, &mut out);
    out
}

/// Rebuild a config JSON object from submitted form values, coercing each
/// field to the type declared in `schema`.
#[must_use]
pub fn form_to_config<S: std::hash::BuildHasher>(
    schema: &Value,
    form: &HashMap<String, String, S>,
) -> Value {
    let mut map = Map::new();
    object_from_form(schema, form, ROOT, &mut map);
    Value::Object(map)
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

fn render_object(schema: &Value, current: &Value, prefix: &str, out: &mut String) {
    let Some(props) = schema.get("properties").and_then(Value::as_object) else {
        return;
    };
    for (key, sub) in props {
        let full = format!("{prefix}.{key}");
        let val = current.get(key).cloned().unwrap_or(Value::Null);
        match primary_type(sub) {
            "boolean" => render_bool(&full, key, &val, out),
            "integer" | "number" => render_number(&full, key, &val, out),
            "object" => {
                let _ = write!(out, "<fieldset><legend>{}</legend>", esc(key));
                render_object(sub, &val, &full, out);
                out.push_str("</fieldset>");
            }
            "array" => render_array(&full, key, &val, out),
            _ => render_string(&full, key, &val, out),
        }
    }
}

fn render_bool(name: &str, label: &str, val: &Value, out: &mut String) {
    let checked = if val.as_bool().unwrap_or(false) {
        " checked"
    } else {
        ""
    };
    let _ = write!(
        out,
        "<label class=\"checkbox\"><input type=\"checkbox\" name=\"{}\" value=\"true\"{}> {}</label>",
        esc(name),
        checked,
        esc(label)
    );
}

fn render_number(name: &str, label: &str, val: &Value, out: &mut String) {
    let v = match val {
        Value::Number(n) => n.to_string(),
        _ => String::new(),
    };
    let _ = write!(
        out,
        "<label>{} <input type=\"number\" step=\"any\" name=\"{}\" value=\"{}\"></label>",
        esc(label),
        esc(name),
        esc(&v)
    );
}

fn render_string(name: &str, label: &str, val: &Value, out: &mut String) {
    let v = val.as_str().unwrap_or("").to_string();
    let _ = write!(
        out,
        "<label>{} <input type=\"text\" name=\"{}\" value=\"{}\"></label>",
        esc(label),
        esc(name),
        esc(&v)
    );
}

fn render_array(name: &str, label: &str, val: &Value, out: &mut String) {
    let v = if val.is_null() {
        "[]".to_string()
    } else {
        serde_json::to_string_pretty(val).unwrap_or_else(|_| "[]".into())
    };
    let _ = write!(
        out,
        "<label>{} (JSON array) <textarea name=\"{}\" rows=\"3\">{}</textarea></label>",
        esc(label),
        esc(name),
        esc(&v)
    );
}

// ---------------------------------------------------------------------------
// Form → JSON
// ---------------------------------------------------------------------------

fn object_from_form<S: std::hash::BuildHasher>(
    schema: &Value,
    form: &HashMap<String, String, S>,
    prefix: &str,
    map: &mut Map<String, Value>,
) {
    let Some(props) = schema.get("properties").and_then(Value::as_object) else {
        return;
    };
    for (key, sub) in props {
        let full = format!("{prefix}.{key}");
        match primary_type(sub) {
            "boolean" => {
                // Unchecked checkboxes are simply absent from the form.
                map.insert(key.clone(), Value::Bool(form.contains_key(&full)));
            }
            "integer" => {
                if let Some(n) = form.get(&full).and_then(|s| s.trim().parse::<i64>().ok()) {
                    map.insert(key.clone(), Value::from(n));
                }
            }
            "number" => {
                if let Some(n) = form.get(&full).and_then(|s| s.trim().parse::<f64>().ok()) {
                    map.insert(key.clone(), Value::from(n));
                }
            }
            "object" => {
                let mut inner = Map::new();
                object_from_form(sub, form, &full, &mut inner);
                map.insert(key.clone(), Value::Object(inner));
            }
            "array" => {
                if let Some(raw) = form.get(&full) {
                    if let Ok(v @ Value::Array(_)) = serde_json::from_str::<Value>(raw) {
                        map.insert(key.clone(), v);
                    }
                }
            }
            _ => {
                if let Some(s) = form.get(&full) {
                    map.insert(key.clone(), Value::String(s.clone()));
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Return the schema node's primary type. Handles `Option<T>` schemas whose
/// `type` is an array like `["string", "null"]` by taking the first non-null.
fn primary_type(sub: &Value) -> &str {
    match sub.get("type") {
        Some(Value::String(s)) => s.as_str(),
        Some(Value::Array(arr)) => arr
            .iter()
            .filter_map(Value::as_str)
            .find(|t| *t != "null")
            .unwrap_or("string"),
        _ => "string",
    }
}

/// Minimal HTML escaping for text content and double-quoted attribute values.
fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn schema() -> Value {
        json!({
            "type": "object",
            "properties": {
                "enabled": { "type": "boolean" },
                "threshold": { "type": "integer" },
                "label": { "type": "string" },
                "nested": {
                    "type": "object",
                    "properties": { "inner": { "type": "string" } }
                }
            }
        })
    }

    #[test]
    fn boolean_renders_checkbox() {
        let html = render_form(&schema(), &json!({ "enabled": true }));
        assert!(html.contains("type=\"checkbox\""));
        assert!(html.contains("name=\"config.enabled\""));
        assert!(html.contains("checked"));
    }

    #[test]
    fn integer_renders_number_input() {
        let html = render_form(&schema(), &json!({ "threshold": 5 }));
        assert!(html.contains("type=\"number\""));
        assert!(html.contains("name=\"config.threshold\""));
        assert!(html.contains("value=\"5\""));
    }

    #[test]
    fn nested_object_renders_fieldset() {
        let html = render_form(&schema(), &json!({}));
        assert!(html.contains("<fieldset>"));
        assert!(html.contains("name=\"config.nested.inner\""));
    }

    #[test]
    fn form_round_trips_to_typed_config() {
        let mut form: HashMap<String, String> = HashMap::new();
        form.insert("config.enabled".into(), "true".into());
        form.insert("config.threshold".into(), "7".into());
        form.insert("config.label".into(), "hi".into());
        form.insert("config.nested.inner".into(), "deep".into());

        let cfg = form_to_config(&schema(), &form);
        assert_eq!(cfg["enabled"], json!(true));
        assert_eq!(cfg["threshold"], json!(7));
        assert_eq!(cfg["label"], json!("hi"));
        assert_eq!(cfg["nested"]["inner"], json!("deep"));
    }

    #[test]
    fn absent_checkbox_is_false() {
        let form: HashMap<String, String> = HashMap::new();
        let cfg = form_to_config(&schema(), &form);
        assert_eq!(cfg["enabled"], json!(false));
    }
}
