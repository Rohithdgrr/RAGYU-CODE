# Problems & Solutions

A record of the significant issues found during the tool-calling review and
how each was fixed. Useful as a checklist for future changes.

## Correctness

| # | Problem | Root cause | Solution |
|---|---|---|---|
| B1 | Text streamed before tool calls was silently dropped — user never saw it, history lost it | tool-round branch only looked at `tool_calls`, discarded `out` | `Message::assistant_with_tool_calls(content, calls)` carries the prose; loop renders it first and commits it with the round |
| B2 | Hostile server could send `"index": 4294967295` → gigabyte `Vec` allocation (OOM) from one SSE line | unbounded `pending_tools.resize(idx + 1)` | `MAX_PARALLEL_TOOL_CALLS = 64`; larger indexes abort the stream with an error; regression test included |
| B3 | Tool-call arguments could grow without bound across unlimited SSE lines | only per-line bytes were capped | `MAX_TOOL_ARGUMENTS_BYTES = 256 KB` checked on every fragment append |
| B4 | Huge tool results stored full-length into history → context/memory blowup | display truncation didn't apply to storage | 8 K-char cap applied before `push_tool_result`, `…(truncated)` marker appended |
| B5 | CI red: `cargo fmt --check` failed on three files after feature work | formatting not run before push | fmt run; full CI gate (`fmt --check && clippy -D warnings && test`) now run locally before every push |
| B6 | Stats inflated: a 3-round tool turn counted as 3 turns, skewing avg latency | `record_turn` called per round | elapsed accumulated across rounds, recorded once per user turn |
| B7 | Raw mode double-printed partial text on errors and prose-before-tools (printed live, then re-printed) | display policy mixed live-stream and end-of-round render paths | extracted `show_round_prose` / `handle_round_error`; raw mode never re-renders already-streamed text |

## Security

| # | Problem | Solution |
|---|---|---|
| S1 | Any registered tool auto-executed whenever the model asked — dangerous once shell tools exist | `/tools` command: registry listing + master on/off switch; off means tools aren't even advertised and rogue server calls are ignored. Per-tool confirm planned for Phase 6 |
| S2 | Executor failure chains (paths, internals) were sent verbatim to the model | model receives sanitized `error: tool '<name>' failed`; details print locally to stderr only |
| S3 | Arguments parsed ad-hoc per builtin, inconsistent errors | shared `parse_args<T: DeserializeOwned>` helper; typed args structs |

## Performance

| # | Problem | Solution |
|---|---|---|
| P1 | Tool specs (JSON schemas) rebuilt via `json!` every round of every turn | built once in `App::new` (`tool_specs`), cloned cheaply per request; `/tools` reads the same cache |

## Code quality

| # | Problem | Solution |
|---|---|---|
| Q1 | Two `#[allow(clippy::too_many_arguments)]` in api.rs | `StreamSink { out, tool_calls }` groups output buffers; allows removed |
| Q2 | Magic numbers scattered (`200` display chars etc.) | named consts (`TOOL_RESULT_DISPLAY_CHARS`, `MAX_TOOL_RESULT_CHARS`, `MAX_TOOL_ROUNDS`) |
| Q3 | `run_turn` ~90 lines mixing transport, display, error policy | split into `chat_options`, `stream_round`, `show_round_prose`, `handle_round_error`, `finish_text_answer`, `run_tool_round` |
| Q4 | Agent loop had no integration test | `tests/tool_loop.rs`: two-phase wiremock test asserting the follow-up request contains assistant prose + `tool_calls` + `role:"tool"` result; plus oversized-result truncation test |
| Q5 | Exported markdown couldn't link results to their calls | `/export md` labels results with `tool_call_id` |
| Q6 | Function calling invisible to users | `/tools` command + `/help` line + README section |

## Process lessons
- Run the exact CI gate locally **before** pushing (fmt is part of it).
- Never trust provider stream fields: every index/size needs a cap.
- When adding an enum field to persisted structs, extend serde with
  defaults + skip-serializing so old files keep loading.
- Display policy ("who prints what, when") should be decided up front;
  retrofitting it caused the raw-mode double-print bug.
