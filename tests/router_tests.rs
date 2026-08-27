//! Router integration tests — wiremock-backed 3-strike failover.

use govinda_cli::provider;
use govinda_cli::router::{FailureKind, Router};

#[test]
fn router_three_strikes_quarantines_and_promotes() {
    let mut r = Router::for_active("omniroute", "auto");
    for _ in 0..3 {
        r.record_failure("auto", FailureKind::Server, "500");
    }
    assert!(r.is_quarantined("auto"));
    let next = r.promote().expect("should promote after quarantine");
    assert_eq!(next.model, "/smart");
}

#[tokio::test]
async fn preflight_probe_handles_success_and_failure() {
    use govinda_cli::preflight::{ProbeStatus, probe_active};
    use wiremock::{Mock, MockServer, ResponseTemplate, matchers};

    // Success case: 200 with content
    let server_ok = MockServer::start().await;
    Mock::given(matchers::method("POST"))
        .and(matchers::path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "choices": [{"message": {"content": "pong"}}]
        })))
        .mount(&server_ok)
        .await;

    let provider = provider::resolve(
        "custom",
        Some(&format!("{}/v1", server_ok.uri())),
        None,
        |_| None,
    )
    .unwrap();
    let http = reqwest::Client::new();
    let ok = probe_active(&http, provider.as_ref(), "test-model").await;
    assert_eq!(ok.status, ProbeStatus::Ok);

    // Failure case: 500
    let server_err = MockServer::start().await;
    Mock::given(matchers::method("POST"))
        .and(matchers::path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server_err)
        .await;
    let provider_err = provider::resolve(
        "custom",
        Some(&format!("{}/v1", server_err.uri())),
        None,
        |_| None,
    )
    .unwrap();
    let err = probe_active(&http, provider_err.as_ref(), "test-model").await;
    assert!(matches!(err.status, ProbeStatus::Err(_)));
}

#[test]
fn auto_compact_and_model_rank_smoke() {
    // auto_compact thresholds: just ensure constants are sane
    assert_eq!(govinda_cli::auto_compact::SOFT_COMPACT_PCT, 90);
    assert_eq!(govinda_cli::auto_compact::HARD_COMPACT_PCT, 98);
    // model_rank returns sorted output for each SortKey
    for sort in [
        govinda_cli::model_rank::SortKey::Quality,
        govinda_cli::model_rank::SortKey::Speed,
        govinda_cli::model_rank::SortKey::Cost,
        govinda_cli::model_rank::SortKey::Context,
        govinda_cli::model_rank::SortKey::Free,
    ] {
        let rows = govinda_cli::model_rank::top_models("omniroute", sort, 3);
        for w in rows.windows(2) {
            assert!(w[0].score >= w[1].score);
        }
    }
    // compress_old_tool_results keeps last 3 untouched
    let msgs = vec![
        govinda_cli::api::Message::tool("c1", "x".repeat(500)),
        govinda_cli::api::Message::tool("c2", "x".repeat(500)),
        govinda_cli::api::Message::tool("c3", "x".repeat(500)),
        govinda_cli::api::Message::tool("c4", "short"),
    ];
    let out = govinda_cli::session::compress_old_tool_results(&msgs);
    assert_eq!(out.len(), msgs.len());
    // provider known_models for omniroute still returns combo ids with correct windows
    let m = provider::known_models("omniroute");
    assert!(
        m.iter()
            .any(|k| k.id == "auto" && k.context_window == 1_048_576)
    );
    assert!(
        m.iter()
            .any(|k| k.id == "/offline" && k.context_window == 32_768)
    );
}
