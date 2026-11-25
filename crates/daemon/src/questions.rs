//! Agents ask for forms as JSON schemas. Clients only lay out fields, so the daemon turns a
//! schema into fields, and the values a client sends back into what the schema asked for.

use protocol::{FieldKind, FieldOption, QuestionField};
use serde_json::{Map, Value};

/// The fields of a form schema, in the order its properties were written. A property of a
/// type this doesn't know comes as [`FieldKind::Unknown`], which clients show as text.
pub fn fields(schema: &Value) -> Vec<QuestionField> {
    let required: Vec<&str> = schema
        .get("required")
        .and_then(Value::as_array)
        .map(|keys| keys.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
        return vec![];
    };
    properties
        .iter()
        .map(|(key, property)| QuestionField {
            key: key.clone(),
            title: text(property, "title"),
            description: text(property, "description"),
            required: required.contains(&key.as_str()),
            kind: kind(property),
        })
        .collect()
}

fn text(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

fn kind(property: &Value) -> FieldKind {
    match property.get("type").and_then(Value::as_str) {
        Some("string") => match choices(property) {
            Some(options) => FieldKind::Choice {
                options,
                multiple: false,
            },
            None => FieldKind::Text,
        },
        Some("array") => match property.get("items").and_then(choices) {
            Some(options) => FieldKind::Choice {
                options,
                multiple: true,
            },
            None => FieldKind::Unknown,
        },
        Some("number") => FieldKind::Number { integer: false },
        Some("integer") => FieldKind::Number { integer: true },
        Some("boolean") => FieldKind::Boolean,
        _ => FieldKind::Unknown,
    }
}

/// The options of a single choice, written as titled `oneOf` or `anyOf` entries, or as a
/// plain `enum` with optional `enumNames`.
fn choices(schema: &Value) -> Option<Vec<FieldOption>> {
    let titled = ["oneOf", "anyOf"]
        .iter()
        .find_map(|key| schema.get(*key).and_then(Value::as_array));
    if let Some(entries) = titled {
        let options: Vec<FieldOption> = entries
            .iter()
            .filter_map(|entry| {
                let value = scalar(entry.get("const")?)?;
                Some(FieldOption {
                    label: text(entry, "title").unwrap_or_else(|| value.clone()),
                    description: text(entry, "description"),
                    value,
                })
            })
            .collect();
        return (!options.is_empty()).then_some(options);
    }
    let values = schema.get("enum").and_then(Value::as_array)?;
    let names = schema.get("enumNames").and_then(Value::as_array);
    let options: Vec<FieldOption> = values
        .iter()
        .enumerate()
        .filter_map(|(index, value)| {
            let value = scalar(value)?;
            let label = names
                .and_then(|names| names.get(index))
                .and_then(Value::as_str)
                .map_or_else(|| value.clone(), str::to_owned);
            Some(FieldOption {
                value,
                label,
                description: None,
            })
        })
        .collect();
    (!options.is_empty()).then_some(options)
}

fn scalar(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

/// The values a client sent, as the fields ask for them. A value that doesn't fit its field
/// is left out, as if it hadn't been answered, rather than sent as something it isn't.
pub fn content(fields: &[QuestionField], values: &Map<String, Value>) -> Map<String, Value> {
    let mut content = Map::new();
    for field in fields {
        let Some(value) = values.get(&field.key) else {
            continue;
        };
        let fitted = match (&field.kind, value) {
            (FieldKind::Choice { multiple: true, .. }, Value::Array(items)) => {
                let items: Vec<Value> =
                    items.iter().filter_map(scalar).map(Value::String).collect();
                (!items.is_empty()).then_some(Value::Array(items))
            }
            (FieldKind::Choice { multiple: true, .. }, other) => {
                scalar(other).map(|item| Value::Array(vec![Value::String(item)]))
            }
            (FieldKind::Number { integer: true }, Value::Number(number)) => number
                .as_i64()
                .or_else(|| number.as_f64().map(|n| n.round() as i64))
                .map(Value::from),
            (FieldKind::Number { integer: true }, Value::String(text)) => {
                text.trim().parse::<i64>().ok().map(Value::from)
            }
            (FieldKind::Number { integer: false }, Value::Number(_)) => Some(value.clone()),
            (FieldKind::Number { integer: false }, Value::String(text)) => text
                .trim()
                .parse::<f64>()
                .ok()
                .and_then(serde_json::Number::from_f64)
                .map(Value::Number),
            (FieldKind::Boolean, Value::Bool(_)) => Some(value.clone()),
            (_, Value::String(text)) if text.trim().is_empty() => None,
            (_, other) => scalar(other).map(Value::String),
        };
        if let Some(fitted) = fitted {
            content.insert(field.key.clone(), fitted);
        }
    }
    content
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// What Claude's adapter sends for AskUserQuestion with two questions.
    fn ask_user_question() -> Value {
        json!({
            "type": "object",
            "properties": {
                "question_0": {
                    "type": "string",
                    "title": "Cache",
                    "description": "Which cache?",
                    "oneOf": [
                        { "const": "Redis", "title": "Redis", "description": "In memory" },
                        { "const": "None", "title": "None" }
                    ]
                },
                "question_0_custom": { "type": "string", "title": "Other" },
                "question_1": {
                    "type": "array",
                    "items": { "anyOf": [
                        { "const": "a", "title": "A" },
                        { "const": "b", "title": "B" }
                    ] }
                }
            }
        })
    }

    #[test]
    fn choices_and_text_become_fields_in_their_order() {
        let fields = fields(&ask_user_question());
        let keys: Vec<&str> = fields.iter().map(|f| f.key.as_str()).collect();
        assert_eq!(keys, ["question_0", "question_0_custom", "question_1"]);
        assert_eq!(fields[0].title.as_deref(), Some("Cache"));
        assert_eq!(
            fields[0].kind,
            FieldKind::Choice {
                options: vec![
                    FieldOption {
                        value: "Redis".into(),
                        label: "Redis".into(),
                        description: Some("In memory".into()),
                    },
                    FieldOption {
                        value: "None".into(),
                        label: "None".into(),
                        description: None,
                    },
                ],
                multiple: false,
            }
        );
        assert_eq!(fields[1].kind, FieldKind::Text);
        assert!(matches!(
            fields[2].kind,
            FieldKind::Choice { multiple: true, .. }
        ));
    }

    #[test]
    fn plain_enums_numbers_and_flags_are_understood() {
        let fields = fields(&json!({
            "type": "object",
            "required": ["n"],
            "properties": {
                "color": { "type": "string", "enum": ["r", "g"], "enumNames": ["Red", "Green"] },
                "n": { "type": "integer" },
                "x": { "type": "number" },
                "ok": { "type": "boolean" },
                "odd": { "type": "object" }
            }
        }));
        let color = fields.iter().find(|f| f.key == "color").unwrap();
        let FieldKind::Choice { options, .. } = &color.kind else {
            panic!("{color:?}");
        };
        assert_eq!(options[1].label, "Green");
        let n = fields.iter().find(|f| f.key == "n").unwrap();
        assert!(n.required);
        assert_eq!(n.kind, FieldKind::Number { integer: true });
        assert_eq!(
            fields.iter().find(|f| f.key == "odd").unwrap().kind,
            FieldKind::Unknown
        );
    }

    #[test]
    fn values_are_fitted_to_their_fields() {
        let fields = fields(&json!({
            "type": "object",
            "properties": {
                "pick": { "type": "array", "items": { "enum": ["a", "b"] } },
                "n": { "type": "integer" },
                "x": { "type": "number" },
                "ok": { "type": "boolean" },
                "note": { "type": "string" },
                "blank": { "type": "string" }
            }
        }));
        let values = json!({
            "pick": "a",
            "n": "42",
            "x": "1.5",
            "ok": true,
            "note": "hi",
            "blank": "  ",
            "stray": "dropped"
        });
        let content = content(&fields, values.as_object().unwrap());
        assert_eq!(
            Value::Object(content),
            json!({ "pick": ["a"], "n": 42, "x": 1.5, "ok": true, "note": "hi" })
        );
    }
}
