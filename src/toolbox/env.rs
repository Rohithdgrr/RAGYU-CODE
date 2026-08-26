//! `env` — read environment variables safely.

use std::collections::BTreeMap;

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Action { Get, Set, List, Unset }

#[derive(Debug, Clone, serde::Deserialize)]
pub struct Args {
    pub action: Action,
    pub key: Option<String>,
    pub value: Option<String>,
}

pub fn run(args: Args) -> anyhow::Result<String> {
    match args.action {
        Action::Get => {
            let k = args.key.ok_or_else(|| anyhow::anyhow!("key required for get"))?;
            match std::env::var(&k) {
                Ok(v) => {
                    let masked = if should_mask(&k) { mask(&v) } else { v };
                    Ok(format!("{{\"key\":\"{}\",\"value\":{}}}", k, serde_json::Value::String(masked)))
                }
                Err(_) => Ok(format!("{{\"key\":\"{}\",\"value\":null,\"exists\":false}}", k)),
            }
        }
        Action::Set => {
            let k = args.key.ok_or_else(|| anyhow::anyhow!("key required for set"))?;
            let v = args.value.ok_or_else(|| anyhow::anyhow!("value required for set"))?;
            // SAFETY: govinda is single-threaded at the point these tools run.
            unsafe { std::env::set_var(&k, &v); }
            Ok(format!("{{\"key\":\"{}\",\"set\":true,\"chars\":{}}}", k, v.chars().count()))
        }
        Action::List => {
            let mut out = BTreeMap::new();
            for (k, v) in std::env::vars() {
                if k.starts_with("GOVINDA") || k.ends_with("_API_KEY") || k.ends_with("_TOKEN") {
                    out.insert(k, mask(&v));
                }
            }
            Ok(format!("{{\"env\":{},\"count\":{}}}", serde_json::to_string(&out).unwrap_or_default(), out.len()))
        }
        Action::Unset => {
            let k = args.key.ok_or_else(|| anyhow::anyhow!("key required for unset"))?;
            // SAFETY: see set_var.
            unsafe { std::env::remove_var(&k); }
            Ok(format!("{{\"key\":\"{}\",\"unset\":true}}", k))
        }
    }
}

/// Mask secrets in `value`, keeping the first 2 and last 1 chars visible.
fn mask(value: &str) -> String {
    if value.chars().count() <= 6 {
        "****".to_string()
    } else {
        let chars: Vec<char> = value.chars().collect();
        let prefix: String = chars.iter().take(2).collect();
        let suffix: String = chars.iter().rev().take(1).collect();
        format!("{prefix}***{suffix}")
    }
}

fn should_mask(key: &str) -> bool {
    let upper = key.to_uppercase();
    upper.contains("KEY") || upper.contains("TOKEN") || upper.contains("SECRET") || upper.contains("PASSWORD")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mask_short_value_fully_redacted() {
        assert_eq!(mask("abc"), "****");
    }

    #[test]
    fn mask_long_value_keeps_edges() {
        let result = mask("sk-1234567890abcdef");
        assert!(result.contains("***"));
    }

    #[test]
    fn get_missing_key_returns_null() {
        let args = Args { action: Action::Get, key: Some("GOVINDA_TEST_MISSING_XYZ".into()), value: None };
        let result = run(args).unwrap();
        assert!(result.contains("\"exists\":false"));
    }
}
