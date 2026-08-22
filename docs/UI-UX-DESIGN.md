# UI/UX Design

govinda-cli is a terminal REPL; "UI" means prompt, streaming output, colors,
and command ergonomics.

## 1. Layout & chrome

```
❯ <your input>                     ← reedline prompt (accent-colored ❯)
… <multiline continuation>         ← multiline indicator
thinking… ⠋                        ← braille spinner while generating (hidden in raw mode)
<streamed / rendered answer>       ← markdown by default, raw tokens with /raw
→ tool_name({...})                 ← dimmed tool invocation line
← {"result": ...}                  ← dimmed one-line truncated tool result
error: …                           ← red stderr line, context chain included
```

- Prompt indicator adapts to vi/emacs mode via reedline.
- Ctrl+C clears the input line or cancels the in-flight reply
  (partial text kept and marked *(interrupted)*); Ctrl+D exits.
- Exit prints a dimmed `bye.` plus autosave notice.

## 2. Rendering modes

| Mode | Behavior | Toggle |
|---|---|---|
| Markdown (default) | spinner during generation; answer rendered once complete via termimad | `/raw off` |
| Raw | token-by-token live printing, no spinner | `/raw on` |

Rule applied everywhere: **never print the same text twice.** In raw mode,
text shown live is never re-rendered — only separators/newlines are added.

## 3. Color system

- `accent()` — prompt, banners (`govinda-cli v0.1.0`), resume notices
- green — user-labeled lines (`[you]`), success confirmations, `/tools` names
- bot color — `[bot]` history lines, assistant content labels
- dim — hints, autosave notes, tool call/result lines, empty responses
- red (stderr) — all errors

Ten themes switch the palette: `default`, `mono`, `dracula`, `solarized`,
`ocean`, `nord`, `gruvbox`, `tokyo-night`, `catppuccin`, `rose`
(`/theme <name>`; `/theme` shows current; lookup is case-insensitive).
Mono exists for non-color terminals/screenshots.

Note on scope: terminal emulators own fonts, font sizes, and background
colors — a CLI app cannot change them (only emit ANSI colors). Themes
therefore control text colors only.

## 4. Tool-calling UX

- Invocation: `→ name(arguments)` in dim color — visible but quiet.
- Result preview: first line, ≤200 chars, prefixed `←`.
- Prose streamed before calls renders like any answer before the tool lines.
- Round cap message when 5 rounds exhaust:
  *stopped after 5 tool rounds — ask again to continue.*
- `/tools` lists registered tools (name highlighted + description);
  `/tools off` gives an explicit confirmation; disabled state says how to
  re-enable.
- Failures: full error chain locally in red; the model sees only a sanitized
  one-liner (no path/secret leakage into the conversation).

## 5. Command ergonomics

- Every command accepts no-arg form to *show current value* instead of
  failing (`/temp`, `/system`, `/theme`, `/model`, `/timeout`, `/limit`).
- Usage strings print exact accepted ranges (e.g. `/timeout <1-600>`).
- Ambiguous `/model` matches list all candidates instead of guessing.
- `/pick` bounds-checks against pending variants; `/variants` validates 1–5.
- Unknown commands suggest `/help`; bare `/` is ignored silently.

## 6. Feedback & error principles

1. Optimistic feedback within one frame: spinner starts immediately.
2. Errors go to **stderr**, answers to stdout — pipe-friendly.
3. Destructive-ish actions confirm outcome (`removed last exchange.`,
   `compacted: folded N messages…`) so users always know what changed.
4. Startup banner states version, provider, and help hint in two lines max.
