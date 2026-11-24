//! Library maintenance, phone listening, and signed releases for String.
pub mod duplicates;
pub mod edit;
pub mod phone;
pub mod spotify;
mod text;
pub mod updates;

use serde_json::Value;
use string_core::Fields;

fn object<'a>(fields: &'a mut Fields, key: &str) -> &'a mut Fields {
    let value = fields.entry(key).or_insert(Value::Null);
    if !value.is_object() {
        *value = Value::Object(Fields::new());
    }
    value.as_object_mut().unwrap()
}
fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64() != Some(0.),
        Value::String(s) => !s.is_empty(),
        _ => true,
    }
}
