//! `json_query` — extract fields from JSON using dot-path queries.
//!
//! Supports: `a.b.c`, `a[0].name`, `a[*].id` (all elements in an array).
//! Drastically reduces token usage when working with large JSON.

use std::path::Path;

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Args {
    /// File path to read, OR inline JSON if `source` is "raw".
    pub source: String,
    /// Dot-path query (e.g. "data.users[0].name").
    pub query: String,
}

pub fn run(args: Args) -> anyhow::Result<String> {
    let value: serde_json::Value = if args.source == "raw" {
        serde_json::from_str(&args.query)
            .map_err(|e| anyhow::anyhow!("inline JSON is not valid: {e}"))?
    } else {
        let path = Path::new(&args.source);
        let raw = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("cannot read '{}': {e}", args.source))?;
        serde_json::from_str(&raw)
            .map_err(|e| anyhow::anyhow!("'{}' is not valid JSON: {e}", args.source))?
    };
    let result = query(&value, &args.query)?;
    let owned: Vec<serde_json::Value> = result.iter().map(|v| (*v).clone()).collect();
    Ok(serde_json::to_string_pretty(&owned).unwrap_or_else(|_| format!("{:#?}", owned)))
}

fn query<'a>(
    value: &'a serde_json::Value,
    path: &str,
) -> anyhow::Result<Vec<&'a serde_json::Value>> {
    let mut results = vec![value];
    let mut buf = String::new();
    let mut in_bracket = false;
    let mut tokens: Vec<String> = Vec::new();
    for ch in path.chars() {
        match ch {
            '.' => {
                if !buf.is_empty() && !in_bracket {
                    tokens.push(buf.clone());
                    buf.clear();
                }
            }
            '[' => {
                if !buf.is_empty() {
                    tokens.push(buf.clone());
                    buf.clear();
                }
                in_bracket = true;
            }
            ']' => {
                in_bracket = false;
                if !buf.is_empty() {
                    tokens.push(buf.clone());
                    buf.clear();
                }
            }
            _ => buf.push(ch),
        }
    }
    if !buf.is_empty() {
        tokens.push(buf);
    }
    for token in tokens {
        if let Ok(idx) = token.parse::<usize>() {
            results = results
                .into_iter()
                .flat_map(|v| v.as_array().and_then(|a| a.get(idx)))
                .collect();
        } else if token == "*" {
            results = results
                .into_iter()
                .flat_map(|v| v.as_array().into_iter().flatten())
                .collect();
        } else {
            results = results.into_iter().flat_map(|v| v.get(&token)).collect();
        }
        if results.is_empty() {
            return Ok(results);
        }
    }
    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn query_simple_field() {
        let v = json!({"a": {"b": 42}});
        let r = query(&v, "a.b").unwrap();
        assert_eq!(r, vec![&json!(42)]);
    }

    #[test]
    fn query_array_index() {
        let v = json!({"items": [{"id": 1}, {"id": 2}]});
        let r = query(&v, "items[1].id").unwrap();
        assert_eq!(r, vec![&json!(2)]);
    }

    #[test]
    fn query_array_wildcard() {
        let v = json!({"items": [{"id": 1}, {"id": 2}, {"id": 3}]});
        let r = query(&v, "items[*].id").unwrap();
        assert_eq!(r.len(), 3);
    }

    #[test]
    fn query_missing_field_returns_empty() {
        let v = json!({"a": 1});
        let r = query(&v, "a.b.c").unwrap();
        assert!(r.is_empty());
    }
}
