//! `http_request` — structured HTTP caller (like curl but with parsed JSON).

#[derive(Debug, Clone, serde::Deserialize)]
pub struct Args {
    pub method: Method,
    pub url: String,
    #[serde(default)]
    pub headers: std::collections::BTreeMap<String, String>,
    pub body: Option<String>,
    pub timeout_secs: Option<u64>,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Method { Get, Post, Put, Delete, Patch }

pub async fn run(args: Args) -> anyhow::Result<String> {
    let timeout = args.timeout_secs.unwrap_or(30).clamp(1, 120);
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(5))
        .timeout(std::time::Duration::from_secs(timeout))
        .redirect(reqwest::redirect::Policy::limited(5))
        .user_agent("Mozilla/5.0 (compatible; govinda-cli/1.0)")
        .build()?;
    let mut req = match args.method {
        Method::Get => client.get(&args.url),
        Method::Post => client.post(&args.url),
        Method::Put => client.put(&args.url),
        Method::Delete => client.delete(&args.url),
        Method::Patch => client.patch(&args.url),
    };
    for (k, v) in &args.headers {
        req = req.header(k, v);
    }
    if let Some(b) = &args.body {
        req = req.body(b.clone());
    }
    let resp = req.send().await?;
    let status = resp.status();
    let headers: std::collections::BTreeMap<String, String> = resp
        .headers()
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_owned()))
        .collect();
    let body = resp.text().await?;
    let body_parsed: serde_json::Value = serde_json::from_str(&body).unwrap_or(serde_json::Value::String(truncate(&body, 10_000)));
    Ok(format!(
        "{{\"ok\":{},\"status\":{},\"headers\":{},\"body\":{}}}",
        status.is_success(),
        status.as_u16(),
        serde_json::to_string(&headers).unwrap_or_default(),
        body_parsed,
    ))
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_owned()
    } else {
        let mut out: String = s.chars().take(max).collect();
        out.push('…');
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn invalid_url_errors_cleanly() {
        let args = Args {
            method: Method::Get,
            url: "http://127.0.0.1:1/".into(),
            headers: Default::default(),
            body: None,
            timeout_secs: Some(2),
        };
        assert!(run(args).await.is_err());
    }
}
