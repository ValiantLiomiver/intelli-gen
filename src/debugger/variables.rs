use serde_json::Value;
use super::DebugError;

#[derive(Clone, Debug)]
pub struct Variable {
    pub name: String,
    pub value: String,
    pub type_name: Option<String>,
    pub variables_reference: i64,
}
#[derive(Clone, Debug)]
pub struct Scope {
    pub name: String,
    pub variables_reference: i64,
    pub expensive: bool,
}
#[derive(Clone, Debug)]
pub struct Evaluation {
    pub result: String,
    pub type_name: Option<String>,
    pub variables_reference: i64,
}
pub(super) fn array<'a>(body: &'a Value, field: &str) -> Result<&'a Vec<Value>, DebugError> {
    body[field].as_array().ok_or_else(|| DebugError::Transport(format!("DAP response omitted {field}")))
}
pub(super) fn id(value: &Value, field: &str) -> Result<i64, DebugError> {
    value[field].as_i64().ok_or_else(|| DebugError::Transport(format!("DAP response omitted {field}")))
}
pub(super) fn text(value: &Value, field: &str) -> String { value[field].as_str().unwrap_or_default().into() }
