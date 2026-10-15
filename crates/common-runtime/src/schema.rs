//! Bounds on an untrusted JSON schema before it reaches the engine's schema-to-grammar compiler
//! (F-01-12: pathological grammars are refused here, in microseconds, not compiled there). One
//! copy for every entry point that accepts a schema: the local API's `response_format` and a
//! peer's `RemoteRequest` (common-network), so the two cannot drift.
use common_core::{CommonError, Result};
use serde_json::Value;

/// Largest schema accepted, as compact JSON.
pub const SCHEMA_MAX_BYTES: usize = 64 * 1024;
/// Deepest nesting of objects and arrays accepted.
pub const SCHEMA_MAX_DEPTH: usize = 32;
/// Largest `{n,m}` quantifier, `minItems`/`maxLength`/... value and `pattern` length accepted.
pub const SCHEMA_MAX_REPEAT: u64 = 1024;

/// `Ok` when `schema` is within the bounds above and every `$ref` is local (`#/...`); otherwise
/// `InvalidRequest` naming the bound.
pub fn check_schema(schema: &Value) -> Result<()> {
    if schema.to_string().len() > SCHEMA_MAX_BYTES {
        return Err(CommonError::invalid(format!(
            "json_schema exceeds {SCHEMA_MAX_BYTES} bytes"
        )));
    }
    check_node(schema, 0)
}

// ponytail: keyword checks apply to any object key, so a *property* literally named `pattern`
// with a long string value is refused too; use a real JSON-schema walker if that ever bites.
fn check_node(v: &Value, depth: usize) -> Result<()> {
    if depth > SCHEMA_MAX_DEPTH {
        return Err(CommonError::invalid(format!(
            "json_schema nests deeper than {SCHEMA_MAX_DEPTH} levels"
        )));
    }
    let bad = |m: String| Err(CommonError::invalid(format!("json_schema: {m}")));
    match v {
        Value::Object(map) => {
            for (k, val) in map {
                match (k.as_str(), val) {
                    ("$ref", Value::String(r)) if !r.starts_with('#') => {
                        return bad("only local $ref (#/...) is supported".into())
                    }
                    ("pattern", Value::String(p))
                        if p.len() as u64 > SCHEMA_MAX_REPEAT || pattern_repeat_too_large(p) =>
                    {
                        return bad(format!(
                            "pattern longer than {SCHEMA_MAX_REPEAT} or with a repetition above {SCHEMA_MAX_REPEAT}"
                        ))
                    }
                    (
                        "minItems" | "maxItems" | "minLength" | "maxLength" | "minProperties" | "maxProperties"
                        | "minContains" | "maxContains",
                        Value::Number(n),
                    ) if n.as_u64().is_none_or(|n| n > SCHEMA_MAX_REPEAT) => {
                        return bad(format!("{k} must be an integer in 0..={SCHEMA_MAX_REPEAT}"))
                    }
                    _ => check_node(val, depth + 1)?,
                }
            }
        }
        Value::Array(items) => {
            for i in items {
                check_node(i, depth + 1)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// True when a regex `{n}` / `{n,}` / `{n,m}` quantifier names a count above `SCHEMA_MAX_REPEAT`.
fn pattern_repeat_too_large(p: &str) -> bool {
    p.split('{').skip(1).filter_map(|rest| rest.split('}').next()).any(|q| {
        q.split(',')
            .any(|n| n.trim().parse::<u64>().is_ok_and(|n| n > SCHEMA_MAX_REPEAT))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use common_core::ErrorCode;
    use serde_json::json;

    #[test]
    fn bombs_are_refused_fast_and_reasonable_schemas_pass() {
        let mut deep = json!({"type": "string"});
        for _ in 0..40 {
            deep = json!({"type": "object", "properties": {"a": deep}});
        }
        let huge =
            json!({"type": "object", "properties": {"a": {"type": "string", "enum": vec!["x".repeat(1000); 80]}}});
        let bombs = [
            (deep, "nests deeper"),
            (json!({"type": "string", "pattern": "^(a{1,100000})$"}), "repetition"),
            (json!({"type": "string", "pattern": "a{2000}"}), "repetition"),
            (json!({"type": "string", "pattern": "x".repeat(2000)}), "pattern longer"),
            (json!({"type": "array", "minItems": 1000000}), "minItems"),
            (json!({"type": "string", "maxLength": -1}), "maxLength"),
            (json!({"type": "string", "maxLength": 1.5}), "maxLength"),
            (json!({"$ref": "https://evil.example/schema.json"}), "local $ref"),
            (
                json!({"type": "object", "properties": {"a": {"$ref": "file:///etc/passwd"}}}),
                "local $ref",
            ),
            (huge, "exceeds"),
        ];
        for (schema, why) in bombs {
            let t = std::time::Instant::now();
            let e = check_schema(&schema).unwrap_err();
            assert_eq!(e.code, ErrorCode::InvalidRequest);
            assert!(e.message.contains(why), "{}", e.message);
            assert!(t.elapsed() < std::time::Duration::from_secs(1));
        }
        let ok = json!({"type": "object", "$defs": {"s": {"type": "string", "pattern": "^[a-z]{1,64}$", "maxLength": 64}}, "properties": {"a": {"$ref": "#/$defs/s"}, "b": {"type": "array", "items": {"type": "integer"}, "minItems": 0, "maxItems": 1024}}, "required": ["a"]});
        check_schema(&ok).unwrap();
        assert!(!pattern_repeat_too_large("^\\d{3}-\\d{4}$"));
        assert!(pattern_repeat_too_large("(ab){1,5000}"));
        assert!(!pattern_repeat_too_large("{unterminated"));
    }
}
