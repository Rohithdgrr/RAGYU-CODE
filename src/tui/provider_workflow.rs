//! Provider setup workflow — extracted from `src/tui/app.rs` (110KB god-module).
//!
//! The TUI previously owned the entire multi-step guided provider setup
//! (select provider → API key → model → test → result) inline, bloating
//! `app.rs` past 2800 lines. This module isolates that state machine so
//! `app.rs` stays focused on event handling and rendering. `App.rs` re-exports
//! `ProviderWorkflow` for backwards compatibility.

/// Multi-step guided provider setup: select provider → API key → model → test.
#[derive(Debug, Clone)]
pub enum ProviderWorkflow {
    /// Step 1: user is selecting a provider from the list.
    SelectProvider {
        providers: Vec<String>,
        selected: usize,
    },
    /// Step 2: user is entering an API key for the chosen provider.
    EnterApiKey {
        provider: String,
        key_input: String,
        cursor: usize,
    },
    /// Step 3: user is selecting a model from the provider.
    SelectModel {
        provider: String,
        api_key: String,
        models: Vec<String>,
        selected: usize,
    },
    /// Step 4: testing the connection with the chosen model.
    Testing {
        provider: String,
        api_key: String,
        model: String,
    },
    /// Step 5: showing the result.
    Result {
        provider: String,
        api_key: String,
        model: String,
        ok: bool,
        message: String,
    },
}
