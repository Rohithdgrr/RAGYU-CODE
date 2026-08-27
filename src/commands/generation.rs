use super::{App, dim, err, info, markdown, ok};
use crate::api;
use futures_util::future::join_all;
use std::sync::Arc;

pub(super) async fn models(arg: &str, app: &mut App) {
    // Parse optional subcommand: `/models top [N] [--sort=key]`.
    let trimmed = arg.trim();
    if let Some(rest) = trimmed.strip_prefix("top") {
        return models_top(rest.trim(), app).await;
    }
    models_list(app).await;
}

async fn models_list(app: &mut App) {
    let provider_id = app.config.provider.key();
    // Try the live API first; fall back to the static registry.
    let api_list = ensure_models(app).await.ok();
    let known = crate::provider::known_models(&provider_id);

    // Merge: API models first (with free tags from registry), then known-only.
    if let Some(ref list) = api_list {
        info(format!("models for {provider_id} (from API):"));
        for id in list.iter() {
            let marker = if **id == app.config.model {
                "  ← current"
            } else {
                ""
            };
            let free_tag = known
                .iter()
                .find(|k| *k.id == **id)
                .map_or("", |k| if k.free { " [FREE]" } else { "" });
            let desc = known
                .iter()
                .find(|k| *k.id == **id)
                .map_or("", |k| k.description);
            let suffix = if !desc.is_empty() || !free_tag.is_empty() {
                format!("  {free_tag}  {desc}")
            } else {
                String::new()
            };
            info(format!("  {id}{marker}{suffix}"));
        }
        // Show known models not in API list
        let api_set: std::collections::HashSet<&str> = list.iter().map(|s| s.as_str()).collect();
        let extra: Vec<&crate::provider::KnownModel> =
            known.iter().filter(|k| !api_set.contains(k.id)).collect();
        if !extra.is_empty() {
            info("");
            info("known models (not listed by API):");
            for m in &extra {
                let marker = if *m.id == app.config.model {
                    "  ← current"
                } else {
                    ""
                };
                let tag = if m.free { " [FREE]" } else { "" };
                info(format!("  {}{tag}{marker}  {}", m.id, m.description));
            }
        }
    } else if !known.is_empty() {
        info(format!("models for {provider_id} (from registry):"));
        for m in known {
            let marker = if m.id == app.config.model {
                "  ← current"
            } else {
                ""
            };
            let tag = if m.free { " [FREE]" } else { "" };
            info(format!("  {}{tag}{marker}  {}", m.id, m.description));
        }
    } else {
        err(format!(
            "no models available for '{provider_id}' — try /models after switching provider",
        ));
    }
}

async fn ensure_models(app: &mut App) -> anyhow::Result<Arc<Vec<String>>> {
    if let Some(list) = &app.models_cache {
        return Ok(Arc::clone(list));
    }
    let url = app.config.provider.models_url().ok_or_else(|| {
        anyhow::anyhow!(
            "provider '{}' has no model-listing endpoint",
            app.config.provider.key()
        )
    })?;
    let list =
        Arc::new(api::list_models(&app.http, &url, app.config.provider.auth().token()).await?);
    app.models_cache = Some(Arc::clone(&list));
    Ok(list)
}

async fn models_top(rest: &str, app: &mut App) {
    let mut n: usize = 5;
    let mut sort_key = crate::model_rank::SortKey::Quality;
    for tok in rest.split_whitespace() {
        if let Some(v) = tok.strip_prefix("--sort=") {
            if let Some(k) = crate::model_rank::SortKey::parse(v) {
                sort_key = k;
            } else {
                err(format!(
                    "unknown sort key '{v}' (use quality|speed|cost|context|free)"
                ));
                return;
            }
        } else if let Ok(v) = tok.parse::<usize>() {
            n = v;
        }
    }
    let provider_key = app.config.provider.key();
    let provider_id: &str = provider_key.as_ref();
    // Prefer the new health-aware ranker when router health is available.
    let rows =
        crate::model_rank::top_models_with_health(provider_id, sort_key, n, Some(&app.router));
    if rows.is_empty() {
        err(format!("no registry models for '{provider_id}'"));
        return;
    }
    info(format!(
        "top {n} models for {provider_id} (sort={sort_key:?}):"
    ));
    for (i, m) in rows.iter().enumerate() {
        let marker = if m.id == app.config.model {
            "  ← current"
        } else {
            ""
        };
        let free_tag = if m.free { " [FREE]" } else { "" };
        let ctx = if m.context_window == 0 {
            "ctx=?".to_owned()
        } else {
            format!("ctx={}", m.context_window)
        };
        info(format!(
            "  {}. {id}{tag}{marker}  ({ctx})  {desc}",
            i + 1,
            id = m.id,
            tag = free_tag,
            desc = m.description
        ));
    }
}

/// Resolves a model name: `next`/`prev` cycle through the cached list, any
/// other value is matched exactly first and then by unique substring.
async fn resolve_model(name: &str, app: &mut App) -> anyhow::Result<Option<String>> {
    let list = ensure_models(app).await?;
    if name == "next" || name == "prev" {
        let pos = list.iter().position(|m| *m == app.config.model);
        let step = if name == "next" { 1 } else { list.len() - 1 };
        let idx = pos.map_or(0, |p| (p + step) % list.len());
        return Ok(list.get(idx).cloned());
    }
    if list.iter().any(|m| m == name) {
        return Ok(Some(name.to_owned()));
    }
    let hits: Vec<&String> = list.iter().filter(|m| m.contains(name)).collect();
    match hits.len() {
        1 => Ok(Some(hits[0].clone())),
        0 => Ok(None),
        _ => {
            err(format!(
                "'{name}' is ambiguous: {}",
                hits.iter()
                    .map(|h| h.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
            Ok(None)
        }
    }
}

pub(super) async fn set_model(name: &str, app: &mut App) {
    if name.is_empty() {
        info(format!(
            "usage: /model <name|next|prev>   (current: {})",
            app.config.model
        ));
        return;
    }
    let requested = name.to_owned();
    match resolve_model(&requested, app).await {
        Ok(Some(full_name)) => {
            app.config.model = full_name.clone();
            ok(format!("model set to {full_name}"));
        }
        Ok(None) => err(format!(
            "unknown model '{requested}' — run /models to see valid ids"
        )),
        Err(e) => {
            // Offline / API hiccup: allow the switch, just don't pretend we checked.
            dim(format!(
                "could not verify against API ({e:#}) — setting anyway."
            ));
            app.config.model = requested.clone();
            ok(format!("model set to {requested}"));
        }
    }
}

/// Drops the last exchange and returns its user text so main can resend it.
pub(super) fn retry(app: &mut App) -> Option<String> {
    let msgs = app.session.messages();
    let idx = msgs.iter().rposition(|m| m.role == "user")?;
    let text = msgs[idx].content.clone();
    app.session.truncate_messages(idx);
    Some(text)
}

/// Builds the summarizer context: system instruction + full history + a
/// trailing user turn. Providers (e.g. Mistral) reject requests whose last
/// message is an assistant turn, which is how histories typically end.
fn compact_context(history: &[api::Message]) -> Vec<api::Message> {
    let mut ctx = vec![api::Message::system(
        "Summarize this conversation into a compact factual brief. \
         Preserve key decisions, facts and open questions. Reply with the summary only.",
    )];
    ctx.extend(history.iter().cloned());
    ctx.push(api::Message::user("Summarize the conversation above now."));
    ctx
}

/// Folds the whole history into a single assistant summary via the API.
/// Public to the crate so `auto_compact` can call it with a swapped
/// summarizer model.
pub(crate) async fn compact(app: &mut App) {
    if app.session.messages().len() < 2 {
        dim("conversation too short to compact.");
        return;
    }
    dim("summarizing conversation…");
    let ctx = compact_context(app.session.messages());
    let auth = app.config.provider.auth();
    let opts = api::ChatOptions {
        max_response_bytes: app.max_response_bytes,
        read_timeout: app.read_timeout,
        ..api::ChatOptions::new(auth.token(), &app.config.model, app.config.temperature)
    };
    let mut out = String::new();
    let mut no_calls = Vec::new();
    let mut sink = api::StreamSink::new(&mut out, &mut no_calls);
    match api::stream_chat(
        &app.http,
        app.config.provider.as_ref(),
        &opts,
        &ctx,
        &mut sink,
        |_| {},
    )
    .await
    {
        Ok(()) if !out.trim().is_empty() => {
            let removed = app.session.compact_with_summary(out.trim());
            ok(format!(
                "compacted: folded {removed} messages into one summary (~{} tokens now).",
                app.session.approx_tokens()
            ));
        }
        Ok(()) => err("the API returned an empty summary — history left untouched."),
        Err(e) => err(format!("compact failed ({e:#}) — history left untouched.")),
    }
}

/// Generates `n` alternate answers for the last user question without
/// committing any of them; use `/pick <n>` to commit one.
///
/// All variants are requested concurrently; results print in order once the
/// whole batch settles.
#[allow(dead_code)]
pub(super) async fn generate_variants(arg: &str, app: &mut App) {
    let n = arg.parse::<usize>().ok().filter(|n| (1..=5).contains(n));
    let n = match n {
        Some(n) => n,
        None => {
            info("usage: /variants <1-5>");
            return;
        }
    };
    if !app.session.messages().iter().any(|m| m.role == "user") {
        dim("nothing to vary yet — ask something first.");
        return;
    }
    let mut ctx = vec![api::Message::system(app.session.system())];
    let keep = app.session.messages().len().saturating_sub(
        if app
            .session
            .messages()
            .last()
            .is_some_and(|m| m.role == "assistant")
        {
            1
        } else {
            0
        },
    );
    ctx.extend(app.session.messages().iter().take(keep).cloned());

    // Clone everything the parallel futures need so `app` is never borrowed
    // across an await point.
    dim(format!("generating {n} variants concurrently…"));
    let model = app.config.model.clone();
    let temperature = (app.config.temperature + 0.2).min(1.0);
    let max_response_bytes = app.max_response_bytes;
    let read_timeout = app.read_timeout;
    let http = app.http.clone();
    let provider = Arc::clone(&app.config.provider);
    let auth = provider.auth();

    let results = join_all((0..n).map(|_| {
        let opts = api::ChatOptions {
            max_response_bytes,
            read_timeout,
            ..api::ChatOptions::new(auth.token(), &model, temperature)
        };
        let http = http.clone();
        let provider = Arc::clone(&provider);
        let ctx = ctx.clone();
        async move {
            let mut out = String::new();
            let mut no_calls = Vec::new();
            let mut sink = api::StreamSink::new(&mut out, &mut no_calls);
            api::stream_chat(&http, provider.as_ref(), &opts, &ctx, &mut sink, |_| {})
                .await
                .map(|_| out)
        }
    }))
    .await;

    for (i, res) in results.into_iter().enumerate() {
        match res {
            Ok(out) if !out.trim().is_empty() => {
                let preview: String = out.trim().lines().next().unwrap_or("").to_owned();
                ok(format!("({}) {}", i + 1, truncate_preview(&preview, 100)));
                app.pending_variants.push(out.trim().to_owned());
            }
            Ok(_) => err(format!("variant {} came back empty.", i + 1)),
            Err(e) => err(format!("variant {} failed ({e:#}).", i + 1)),
        }
    }
    dim("type /pick <n> to commit one of these, or just keep chatting to discard them.");
}

#[allow(dead_code)]
pub(super) fn pick_variant(arg: &str, app: &mut App) {
    let idx: usize = match arg.trim().parse::<usize>() {
        Ok(i) if i >= 1 && i <= app.pending_variants.len() => i - 1,
        _ => {
            info(format!(
                "usage: /pick <1-{}>",
                app.pending_variants.len().max(1)
            ));
            return;
        }
    };
    let chosen = app.pending_variants.swap_remove(idx);
    // Replace the trailing assistant answer (if any) with the chosen variant.
    if app
        .session
        .messages()
        .last()
        .is_some_and(|m| m.role == "assistant")
    {
        app.session
            .truncate_messages(app.session.messages().len() - 1);
    }
    app.session.push_assistant(chosen.clone());
    app.pending_variants.clear();
    markdown(chosen);
    ok("variant committed.");
}

#[allow(dead_code)]
fn truncate_preview(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_owned()
    } else {
        let cut: String = s.chars().take(max_chars).collect();
        format!("{cut}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_context_always_ends_on_a_user_turn() {
        // Real histories typically end with an assistant reply; Mistral
        // rejects summarization requests whose last role is assistant.
        let history = vec![
            api::Message::user("q"),
            api::Message::assistant("a"),
            api::Message::assistant_with_tool_calls(
                "",
                vec![crate::api::ToolCall::new("c", "f", "{}")],
            ),
            api::Message::tool("c", "result"),
        ];
        let ctx = compact_context(&history);
        assert!(matches!(ctx.last(), Some(m) if m.role == "user"));
        assert_eq!(ctx.first().map(|m| m.role.as_str()), Some("system"));
        assert_eq!(ctx.len(), history.len() + 2);
    }

    /// Functional `/compact` test: the API summarizes the whole history and
    /// the session folds down to that summary — no naive truncation, no data
    /// loss beyond what the summary preserves.
    #[tokio::test]
    async fn compact_folds_history_into_api_summary() {
        let server = wiremock::MockServer::start().await;
        let body = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"BRIEF: user asked x; answer was y.\"}}]}\n\n",
            "data: [DONE]\n\n",
        );
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/v1/chat/completions"))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(body),
            )
            .expect(1)
            .mount(&server)
            .await;

        let mut app = super::super::tests::smoke_app();
        app.config.provider = crate::provider::resolve(
            "custom",
            Some(&format!("{}/v1", server.uri())),
            None,
            |_| None,
        )
        .expect("custom provider");

        for i in 0..5 {
            app.session.push_user(format!("question {i}"));
            app.session.push_assistant(format!("answer {i}"));
        }
        let before = app.session.messages().len();
        assert_eq!(before, 10);

        compact(&mut app).await;

        let after = app.session.messages().len();
        assert!(after < before, "history must shrink: {before} -> {after}");
        assert!(after >= 1);
        let last = &app.session.messages()[after - 1];
        assert_eq!(last.role, "assistant");
        assert!(
            last.content.contains("BRIEF"),
            "summary must come from the API reply, got: {}",
            last.content
        );
    }
}
