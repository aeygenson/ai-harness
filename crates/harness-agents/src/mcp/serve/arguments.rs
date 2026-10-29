//! Reading a tool's arguments from the JSON the agent sent, with a clear
//! refusal when one is missing or of the wrong kind.

use serde_json::Value;

use super::tools::CallError;

/// A text argument the tool cannot work without.
pub(super) fn text_argument<'a>(arguments: &'a Value, name: &str) -> Result<&'a str, CallError> {
    optional_text_argument(arguments, name)?
        .ok_or_else(|| CallError::BadArguments(format!("the argument {name:?} is missing")))
}

/// A text argument that may be left out.
pub(super) fn optional_text_argument<'a>(
    arguments: &'a Value,
    name: &str,
) -> Result<Option<&'a str>, CallError> {
    match arguments.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => Ok(Some(text)),
        Some(_) => Err(CallError::BadArguments(format!("{name:?} must be text"))),
    }
}

/// A count the tool cannot work without, such as `seen_steps`.
pub(super) fn count_argument(arguments: &Value, name: &str) -> Result<usize, CallError> {
    let count = number_argument(arguments, name)?
        .ok_or_else(|| CallError::BadArguments(format!("the argument {name:?} is missing")))?;
    usize::try_from(count)
        .map_err(|_too_big| CallError::BadArguments(format!("{name:?} is too big")))
}

/// A yes-or-no argument; left out means no.
pub(super) fn bool_argument(arguments: &Value, name: &str) -> Result<bool, CallError> {
    match arguments.get(name) {
        None | Some(Value::Null) => Ok(false),
        Some(Value::Bool(value)) => Ok(*value),
        Some(_) => Err(CallError::BadArguments(format!(
            "{name:?} must be true or false"
        ))),
    }
}

/// A whole-number argument that may be left out.
pub(super) fn number_argument(arguments: &Value, name: &str) -> Result<Option<u64>, CallError> {
    match arguments.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value.as_u64().map(Some).ok_or_else(|| {
            CallError::BadArguments(format!("{name:?} must be a whole number of at least 0"))
        }),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn arguments_of_the_wrong_kind_are_refused() {
        let arguments = json!({ "task": 7, "step": "two" });
        assert!(matches!(
            text_argument(&arguments, "task"),
            Err(CallError::BadArguments(_))
        ));
        assert!(matches!(
            number_argument(&arguments, "step"),
            Err(CallError::BadArguments(_))
        ));
        assert!(matches!(
            text_argument(&json!({}), "task"),
            Err(CallError::BadArguments(_))
        ));
        assert_eq!(number_argument(&json!({}), "step"), Ok(None));
        assert_eq!(bool_argument(&json!({}), "now"), Ok(false));
        assert!(matches!(
            bool_argument(&json!({ "now": "yes" }), "now"),
            Err(CallError::BadArguments(_))
        ));
    }
}
