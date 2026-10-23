//! Structural backward-compatibility checks between two OpenAPI 3.1 documents.
//!
//! This is a conservative structural check (removed operations, newly required inputs, removed
//! or retyped response properties). Semantic compatibility still needs human review.

use std::collections::BTreeSet;

use serde_json::Value;

const METHODS: [&str; 8] = [
    "get", "put", "post", "delete", "options", "head", "patch", "trace",
];

/// Returns human-readable breaking changes of `new` relative to `old`; empty means compatible.
pub fn breaking_changes(old: &Value, new: &Value) -> Vec<String> {
    let mut out = Vec::new();
    let Some(old_paths) = old.get("paths").and_then(Value::as_object) else {
        return out;
    };
    let new_paths = new.get("paths").and_then(Value::as_object);
    for (path, old_item) in old_paths {
        for m in METHODS {
            let Some(old_op) = old_item.get(m) else {
                continue;
            };
            let Some(new_op) = new_paths.and_then(|p| p.get(path)).and_then(|i| i.get(m)) else {
                out.push(format!("operation removed: {} {path}", m.to_uppercase()));
                continue;
            };
            let at = format!("{} {path}", m.to_uppercase());
            check_params(old_op, new_op, &at, &mut out);
            check_request_body(old, new, old_op, new_op, &at, &mut out);
            check_responses(old, new, old_op, new_op, &at, &mut out);
        }
    }
    out
}

fn params(op: &Value) -> Vec<(String, String, bool)> {
    op.get("parameters")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|p| {
                    Some((
                        p.get("name")?.as_str()?.to_string(),
                        p.get("in")?.as_str()?.to_string(),
                        p.get("required").and_then(Value::as_bool).unwrap_or(false),
                    ))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn check_params(old: &Value, new: &Value, at: &str, out: &mut Vec<String>) {
    let old_p = params(old);
    for (name, loc, required) in params(new) {
        if required && !old_p.iter().any(|(n, l, _)| *n == name && *l == loc) {
            out.push(format!("{at}: new required {loc} parameter `{name}`"));
        }
    }
}

/// Follow a local `$ref` (`#/components/...`).
fn resolve<'a>(root: &'a Value, node: &'a Value) -> &'a Value {
    let mut cur = node;
    for _ in 0..16 {
        match cur
            .get("$ref")
            .and_then(Value::as_str)
            .and_then(|r| r.strip_prefix('#'))
        {
            Some(ptr) => match root.pointer(ptr) {
                Some(n) => cur = n,
                None => return cur,
            },
            None => return cur,
        }
    }
    cur
}

fn json_schema<'a>(root: &'a Value, container: &'a Value) -> Option<&'a Value> {
    let c = resolve(root, container);
    let s = c.get("content")?.get("application/json")?.get("schema")?;
    Some(resolve(root, s))
}

fn required_set(s: &Value) -> BTreeSet<String> {
    s.get("required")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn check_request_body(
    old_root: &Value,
    new_root: &Value,
    old: &Value,
    new: &Value,
    at: &str,
    out: &mut Vec<String>,
) {
    let (ob, nb) = (old.get("requestBody"), new.get("requestBody"));
    if let (None, Some(nb)) = (ob, nb) {
        if resolve(new_root, nb)
            .get("required")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            out.push(format!("{at}: request body became required"));
        }
        return;
    }
    let (Some(ob), Some(nb)) = (ob, nb) else {
        return;
    };
    if let (Some(os), Some(ns)) = (json_schema(old_root, ob), json_schema(new_root, nb)) {
        for r in required_set(ns).difference(&required_set(os)) {
            out.push(format!("{at}: request property `{r}` became required"));
        }
    }
}

fn check_responses(
    old_root: &Value,
    new_root: &Value,
    old: &Value,
    new: &Value,
    at: &str,
    out: &mut Vec<String>,
) {
    let Some(old_r) = old.get("responses").and_then(Value::as_object) else {
        return;
    };
    let new_r = new.get("responses").and_then(Value::as_object);
    for (code, old_resp) in old_r {
        let Some(new_resp) = new_r.and_then(|r| r.get(code)) else {
            out.push(format!("{at}: response {code} removed"));
            continue;
        };
        if let (Some(os), Some(ns)) = (
            json_schema(old_root, old_resp),
            json_schema(new_root, new_resp),
        ) {
            compare_response_schema(
                old_root,
                new_root,
                os,
                ns,
                &format!("{at} response {code}"),
                out,
                0,
            );
        }
    }
}

fn compare_response_schema(
    old_root: &Value,
    new_root: &Value,
    old: &Value,
    new: &Value,
    at: &str,
    out: &mut Vec<String>,
    depth: u8,
) {
    if depth > 6 {
        return;
    }
    if let (Some(ot), Some(nt)) = (old.get("type"), new.get("type"))
        && ot != nt
    {
        out.push(format!("{at}: type changed from {ot} to {nt}"));
        return;
    }
    let (Some(op), Some(np)) = (
        old.get("properties").and_then(Value::as_object),
        new.get("properties").and_then(Value::as_object),
    ) else {
        if old.get("properties").is_some() && new.get("properties").is_none() {
            out.push(format!("{at}: properties removed"));
        }
        return;
    };
    for (name, old_prop) in op {
        match np.get(name) {
            None => out.push(format!("{at}: property `{name}` removed")),
            Some(new_prop) => compare_response_schema(
                old_root,
                new_root,
                resolve(old_root, old_prop),
                resolve(new_root, new_prop),
                &format!("{at}.{name}"),
                out,
                depth + 1,
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(s: &str) -> Value {
        serde_yaml::from_str(s).unwrap()
    }

    const BASE: &str = r#"
openapi: 3.1.0
info: {title: t, version: '1'}
paths:
  /echo:
    post:
      requestBody:
        required: true
        content: {application/json: {schema: {type: object, required: [message], properties: {message: {type: string}}}}}
      responses:
        '200':
          description: ok
          content: {application/json: {schema: {type: object, properties: {message: {type: string}}}}}
"#;

    #[test]
    fn identical_is_compatible() {
        assert!(breaking_changes(&doc(BASE), &doc(BASE)).is_empty());
    }

    #[test]
    fn additive_is_compatible() {
        let new = BASE.replace("content: {application/json: {schema: {type: object, properties: {message: {type: string}}}}}", "content: {application/json: {schema: {type: object, properties: {message: {type: string}, at: {type: string}}}}}")
            + "  /ping:\n    get:\n      responses: {'200': {description: ok}}\n";
        assert!(
            breaking_changes(&doc(BASE), &doc(&new)).is_empty(),
            "{:?}",
            breaking_changes(&doc(BASE), &doc(&new))
        );
    }

    #[test]
    fn removed_operation_is_breaking() {
        let new = "openapi: 3.1.0\ninfo: {title: t, version: '1'}\npaths: {}\n";
        assert_eq!(breaking_changes(&doc(BASE), &doc(new)).len(), 1);
    }

    #[test]
    fn newly_required_request_property_is_breaking() {
        let new = BASE.replace("required: [message]", "required: [message, other]");
        assert!(breaking_changes(&doc(BASE), &doc(&new))[0].contains("became required"));
    }

    #[test]
    fn removed_response_property_is_breaking() {
        let new = BASE.replace("content: {application/json: {schema: {type: object, properties: {message: {type: string}}}}}", "content: {application/json: {schema: {type: object, properties: {}}}}");
        let b = breaking_changes(&doc(BASE), &doc(&new));
        assert!(b.iter().any(|x| x.contains("`message` removed")), "{b:?}");
    }
}
