use super::{App, dim, err, ok};
use crate::api;
use crate::render::paint;
use crossterm::style::Color;
use futures_util::future::join_all;
use std::sync::Arc;

pub(super) async fn models(app: &mut App) {
    match ensure_models(app).await {
        Ok(list) => {
            println!("available models:");
            for id in list.iter() {
                let marker = if **id == app.config.model {
                    "  ← current"
                } else {
                    ""
                };
                println!("  {id}{marker}");
            }
        }
        Err(e) => err(&format!("{e:#}")),
    }
}

async fn ensure_models(app: &mut App) -> anyhow::Result<Arc<Vec<String>>> {
    if let Some(list) = &app.models_cache {
        return Ok(Arc::clone(list));
    }
    let url = app.config.provider.models_url().ok_or_else(|| {
        anyhow::anyhow!(
            "provider '{}' has no model-listing endpoint",
            app.config.provider.id()
        )
    })?;
    let list =
        Arc::new(api::list_models(&app.http, &url, app.config.provider.auth().token()).await?);
    app.models_cache = Some(Arc::clone(&list));
    Ok(list)
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
            err(&format!(
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
        println!(
            "usage: /model <name|next|prev>   (current: {})",
            app.config.model
        );
        return;
    }
    let requested = name.to_owned();
    match resolve_model(&requested, app).await {
        Ok(Some(full_name)) => {
            app.config.model = full_name.clone();
            ok(&format!("model set to {full_name}"));
        }
        Ok(None) => err(&format!(
            "unknown model '{requested}' — run /models to see valid ids"
        )),
        Err(e) => {
            // Offline / API hiccup: allow the switch, just don't pretend we checked.
            dim(&format!(
                "could not verify against API ({e:#}) — setting anyway."
            ));
            app.config.model = requested.clone();
            ok(&format!("model set to {requested}"));
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

/// Folds the whole history into a single assistant summary via the API.
pub(super) async fn compact(app: &mut App) {
    if app.session.messages().len() < 2 {
        dim("conversation too short to compact.");
        return;
    }
    dim("summarizing conversation…");
    let mut ctx = vec![api::Message::system(
        "Summarize this conversation into a compact factual brief. \
         Preserve key decisions, facts and open questions. Reply with the summary only.",
    )];
    ctx.extend(app.session.messages().iter().cloned());
    let auth = app.config.provider.auth();
    let opts = api::ChatOptions {
        max_response_bytes: app.max_response_bytes,
        read_timeout: app.read_timeout,
        ..api::ChatOptions::new(auth.token(), &app.config.model, app.config.temperature)
    };
    let mut out = String::new();
    match api::stream_chat(
        &app.http,
        app.config.provider.as_ref(),
        &opts,
        &ctx,
        &mut out,
        &mut Vec::new(),
        |_| {},
    )
    .await
    {
        Ok(()) if !out.trim().is_empty() => {
            let removed = app.session.compact_with_summary(out.trim());
            ok(&format!(
                "compacted: folded {removed} messages into one summary (~{} tokens now).",
                app.session.approx_tokens()
            ));
        }
        Ok(()) => err("the API returned an empty summary — history left untouched."),
        Err(e) => err(&format!("compact failed ({e:#}) — history left untouched.")),
    }
}

/// Generates `n` alternate answers for the last user question without
/// committing any of them; use `/pick <n>` to commit one.
///
/// All variants are requested concurrently; results print in order once the
/// whole batch settles.
pub(super) async fn generate_variants(arg: &str, app: &mut App) {
    let n = arg.parse::<usize>().ok().filter(|n| (1..=5).contains(n));
    let n = match n {
        Some(n) => n,
        None => {
            println!("usage: /variants <1-5>");
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
    dim(&format!("generating {n} variants concurrently…"));
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
            api::stream_chat(&http, provider.as_ref(), &opts, &ctx, &mut out, &mut Vec::new(), |_| {})
                .await
                .map(|_| out)
        }
    }))
    .await;

    for (i, res) in results.into_iter().enumerate() {
        match res {
            Ok(out) if !out.trim().is_empty() => {
                let preview: String = out.trim().lines().next().unwrap_or("").to_owned();
                println!(
                    "{} {}",
                    paint(format!("({})", i + 1), Color::Green),
                    truncate_preview(&preview, 100)
                );
                app.pending_variants.push(out.trim().to_owned());
            }
            Ok(_) => err(&format!("variant {} came back empty.", i + 1)),
            Err(e) => err(&format!("variant {} failed ({e:#}).", i + 1)),
        }
    }
    dim("type /pick <n> to commit one of these, or just keep chatting to discard them.");
}

pub(super) fn pick_variant(arg: &str, app: &mut App) {
    let idx: usize = match arg.trim().parse::<usize>() {
        Ok(i) if i >= 1 && i <= app.pending_variants.len() => i - 1,
        _ => {
            println!("usage: /pick <1-{}>", app.pending_variants.len().max(1));
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
    println!();
    app.renderer.render_answer(&chosen);
    ok("variant committed.");
}

fn truncate_preview(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_owned()
    } else {
        let cut: String = s.chars().take(max_chars).collect();
        format!("{cut}…")
    }
}
