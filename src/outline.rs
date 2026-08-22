//! Lightweight, regex-based symbol outlines for `read_file`.
//!
//! The goal is navigation, not compilation: enough structure (functions,
//! types, imports with line numbers) for the model to jump around a file it
//! has only partially read. Heuristics deliberately trade precision for zero
//! heavy dependencies and cross-language coverage; the model re-reads any
//! region it cares about anyway.

use regex::Regex;
use std::sync::OnceLock;

/// Maximum symbols / imports listed before the outline is cut off.
const MAX_SYMBOLS: usize = 150;
const MAX_IMPORTS: usize = 50;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    Rust,
    Python,
    JavaScript,
}

/// Detects the language from a path's extension.
pub fn detect_language(path: &str) -> Option<Language> {
    let ext = path.rsplit('.').next()?;
    match ext.to_ascii_lowercase().as_str() {
        "rs" => Some(Language::Rust),
        "py" => Some(Language::Python),
        "js" | "jsx" | "ts" | "tsx" | "mjs" | "cjs" => Some(Language::JavaScript),
        _ => None,
    }
}

/// A symbol or import entry: 1-based line number plus display text.
struct Entry {
    line: usize,
    label: String,
}

/// Builds a formatted outline for `text`. Returns an empty string when no
/// symbols are recognized (callers then omit the section entirely).
pub fn outline(lang: Language, text: &str) -> String {
    let (mut symbols, mut imports) = match lang {
        Language::Rust => rust_outline(text),
        Language::Python => python_outline(text),
        Language::JavaScript => js_outline(text),
    };

    if symbols.is_empty() && imports.is_empty() {
        return String::new();
    }

    let mut out = String::from("[outline]\n");
    if !symbols.is_empty() {
        out.push_str("defined:\n");
        let truncated = symbols.len() > MAX_SYMBOLS;
        symbols.truncate(MAX_SYMBOLS);
        append_entries(&mut out, &symbols);
        if truncated {
            out.push_str(&format!(
                "…(capped at {MAX_SYMBOLS} symbols — use offset_line to explore)\n"
            ));
        }
    }
    if !imports.is_empty() {
        out.push_str("imports:\n");
        imports.truncate(MAX_IMPORTS);
        append_entries(&mut out, &imports);
    }
    out
}

fn append_entries(out: &mut String, entries: &[Entry]) {
    for e in entries {
        out.push_str(&format!("{:>5}| {}\n", e.line, e.label));
    }
}

// -- Rust -------------------------------------------------------------------

fn rust_res() -> &'static RustRes {
    static RES: OnceLock<RustRes> = OnceLock::new();
    RES.get_or_init(RustRes::new)
}

struct RustRes {
    function: Regex,
    type_item: Regex,
    module: Regex,
    macro_rules: Regex,
    impl_block: Regex,
    use_stmt: Regex,
}

impl RustRes {
    #[allow(clippy::expect_used)] // static, hand-checked patterns
    fn new() -> Self {
        let vis = r"(?:pub(?:\([^)]*\))?\s+)?";
        let prefixes = r#"(?:const\s+)?(?:async\s+)?(?:unsafe\s+)?(?:extern\s+"[^"]*"\s+)?"#;
        let ident = r"[A-Za-z_][A-Za-z0-9_]*";
        Self {
            function: Regex::new(&format!(r"(?m)^{vis}{prefixes}fn\s+(?P<label>{ident})"))
                .expect("valid regex"),
            type_item: Regex::new(&format!(
                r"(?m)^{vis}(?P<kind>struct|enum|union|trait)\s+(?P<label>{ident})"
            ))
            .expect("valid regex"),
            module: Regex::new(&format!(r"(?m)^{vis}mod\s+(?P<label>{ident})"))
                .expect("valid regex"),
            macro_rules: Regex::new(&format!(r"(?m)^\s*macro_rules!\s*(?P<label>{ident})"))
                .expect("valid regex"),
            impl_block: Regex::new(r"(?m)^\s*impl\b(?P<rest>[^\n{]*)").expect("valid regex"),
            use_stmt: Regex::new(r"(?m)^[ \t]*use\s+(?P<rest>[^;\n]+);?").expect("valid regex"),
        }
    }
}

fn rust_outline(text: &str) -> (Vec<Entry>, Vec<Entry>) {
    let r = rust_res();
    let line_at = |pos: usize| text[..pos].matches('\n').count() + 1;
    let mut symbols = Vec::new();
    for cap in r.function.captures_iter(text) {
        symbols.push(Entry {
            line: line_at(cap.get(0).map(|m| m.start()).unwrap_or(0)),
            label: format!("fn {}", &cap["label"]),
        });
    }
    for cap in r.type_item.captures_iter(text) {
        symbols.push(Entry {
            line: line_at(cap.get(0).map(|m| m.start()).unwrap_or(0)),
            label: format!("{} {}", &cap["kind"], &cap["label"]),
        });
    }
    for cap in r.module.captures_iter(text) {
        symbols.push(Entry {
            line: line_at(cap.get(0).map(|m| m.start()).unwrap_or(0)),
            label: format!("mod {}", &cap["label"]),
        });
    }
    for cap in r.macro_rules.captures_iter(text) {
        symbols.push(Entry {
            line: line_at(cap.get(0).map(|m| m.start()).unwrap_or(0)),
            label: format!("macro_rules! {}", &cap["label"]),
        });
    }
    for cap in r.impl_block.captures_iter(text) {
        let rest: String = cap["rest"].split_whitespace().collect::<Vec<_>>().join(" ");
        if !rest.is_empty() {
            symbols.push(Entry {
                line: line_at(cap.get(0).map(|m| m.start()).unwrap_or(0)),
                label: format!("impl {rest}"),
            });
        }
    }

    let imports: Vec<Entry> = r
        .use_stmt
        .captures_iter(text)
        .map(|cap| Entry {
            line: line_at(cap.get(0).map(|m| m.start()).unwrap_or(0)),
            label: format!("use {}", cap["rest"].trim().trim_end_matches(';')),
        })
        .collect();

    symbols.sort_by_key(|e| e.line);
    (symbols, imports)
}

// -- Python -----------------------------------------------------------------

#[allow(clippy::expect_used)] // static, hand-checked patterns
fn python_outline(text: &str) -> (Vec<Entry>, Vec<Entry>) {
    static DEF: OnceLock<Regex> = OnceLock::new();
    static CLASS: OnceLock<Regex> = OnceLock::new();
    static IMPORT: OnceLock<Regex> = OnceLock::new();
    let def = DEF.get_or_init(|| {
        Regex::new(r"(?m)^[ \t]*(?:async\s+)?def\s+([A-Za-z_][A-Za-z0-9_]*)").expect("valid regex")
    });
    let class = CLASS.get_or_init(|| {
        Regex::new(r"(?m)^[ \t]*class\s+([A-Za-z_][A-Za-z0-9_]*)").expect("valid regex")
    });
    let import = IMPORT.get_or_init(|| {
        Regex::new(r"(?m)^[ \t]*(?:import\s+.+|from\s+.+\s+import\s+.+)").expect("valid regex")
    });

    let line_at = |text: &str, pos: usize| text[..pos].matches('\n').count() + 1;
    let mut symbols: Vec<Entry> = def
        .captures_iter(text)
        .map(|c| Entry {
            line: line_at(text, c.get(0).map(|m| m.start()).unwrap_or(0)),
            label: format!("def {}", &c[1]),
        })
        .chain(class.captures_iter(text).map(|c| Entry {
            line: line_at(text, c.get(0).map(|m| m.start()).unwrap_or(0)),
            label: format!("class {}", &c[1]),
        }))
        .collect();
    symbols.sort_by_key(|e| e.line);

    let imports: Vec<Entry> = import
        .find_iter(text)
        .map(|m| Entry {
            line: line_at(text, m.start()),
            label: m.as_str().trim().chars().take(100).collect(),
        })
        .collect();
    (symbols, imports)
}

// -- JavaScript / TypeScript ------------------------------------------------

fn js_res() -> &'static JsRes {
    static RES: OnceLock<JsRes> = OnceLock::new();
    RES.get_or_init(JsRes::new)
}

struct JsRes {
    function: Regex,
    class: Regex,
    fn_assignment: Regex,
    import: Regex,
}

impl JsRes {
    #[allow(clippy::expect_used)] // static, hand-checked patterns
    fn new() -> Self {
        Self {
            function: Regex::new(
                r"(?m)^(?:export\s+)?(?:default\s+)?(?:async\s+)?function\s*\*?\s*([A-Za-z_$][A-Za-z0-9_$]*)",
            )
            .expect("valid regex"),
            class: Regex::new(
                r"(?m)^(?:export\s+)?(?:default\s+)?(?:abstract\s+)?class\s+([A-Za-z_$][A-Za-z0-9_$]*)",
            )
            .expect("valid regex"),
            fn_assignment: Regex::new(
                r"(?m)^(?:export\s+)?(?:const|let|var)\s+([A-Za-z_$][A-Za-z0-9_$]*)\s*(?::[^=\n]+)?=\s*(?:async\s+)?(?:function\b|\([^)\n]*\)\s*=>|[A-Za-z_$][A-Za-z0-9_$]*\s*=>)",
            )
            .expect("valid regex"),
            import: Regex::new(r"(?m)^[ \t]*import\b.*").expect("valid regex"),
        }
    }
}

#[allow(clippy::expect_used)] // static, hand-checked patterns
fn js_outline(text: &str) -> (Vec<Entry>, Vec<Entry>) {
    let r = js_res();
    let line_at = |pos: usize| text[..pos].matches('\n').count() + 1;
    let mut symbols: Vec<Entry> = Vec::new();
    for cap in r.function.captures_iter(text) {
        symbols.push(Entry {
            line: line_at(cap.get(0).map(|m| m.start()).unwrap_or(0)),
            label: format!("function {}", &cap[1]),
        });
    }
    for cap in r.class.captures_iter(text) {
        symbols.push(Entry {
            line: line_at(cap.get(0).map(|m| m.start()).unwrap_or(0)),
            label: format!("class {}", &cap[1]),
        });
    }
    for cap in r.fn_assignment.captures_iter(text) {
        symbols.push(Entry {
            line: line_at(cap.get(0).map(|m| m.start()).unwrap_or(0)),
            label: format!("fn {}", &cap[1]),
        });
    }
    symbols.sort_by_key(|e| e.line);

    let imports: Vec<Entry> = r
        .import
        .find_iter(text)
        .map(|m| Entry {
            line: line_at(m.start()),
            label: m.as_str().trim().chars().take(100).collect(),
        })
        .collect();
    (symbols, imports)
}

#[cfg(test)]
mod tests {
    use super::*;

    const RUST_SRC: &str = "\
use std::collections::HashMap;

pub struct Config {
    pub name: String,
}

pub enum Mode { Fast, Slow }

trait Runner {
    fn run(&self);
}

pub(crate) async fn start(cfg: &Config) {}

impl Runner for Config {
    fn run(&self) {}
}

macro_rules! shout {
    () => {};
}
";

    #[test]
    fn detects_language_by_extension() {
        assert_eq!(detect_language("src/lib.rs"), Some(Language::Rust));
        assert_eq!(detect_language("app.py"), Some(Language::Python));
        assert_eq!(detect_language("a/b/c.TS"), Some(Language::JavaScript));
        assert_eq!(detect_language("notes.txt"), None);
        assert_eq!(detect_language("Makefile"), None);
    }

    #[test]
    fn rust_outline_lists_symbols_and_imports_with_lines() {
        let out = outline(Language::Rust, RUST_SRC);
        assert!(out.contains("[outline]"), "{out}");
        assert!(out.contains("defined:"), "{out}");
        assert!(out.contains("| struct Config"), "{out}");
        assert!(out.contains("| enum Mode"), "{out}");
        assert!(out.contains("| trait Runner"), "{out}");
        assert!(out.contains("| fn start"), "{out}");
        assert!(out.contains("| impl Runner for Config"), "{out}");
        assert!(out.contains("macro_rules! shout"), "{out}");
        assert!(
            out.contains("imports:\n    1| use std::collections::HashMap\n"),
            "{out}"
        );
        // line numbers are accurate
        assert!(out.contains("\n    3| struct Config"), "{out}");
    }

    #[test]
    fn python_outline_lists_defs_classes_imports() {
        let src = "import os\n\n\nclass Dog:\n    def bark(self):\n        pass\n\n    async def fetch(self):\n        pass\n\n\ndef main():\n    Dog().bark()\n";
        let out = outline(Language::Python, src);
        assert!(out.contains("| class Dog"), "{out}");
        assert!(out.contains("| def bark"), "{out}");
        assert!(out.contains("| def fetch"), "{out}");
        assert!(out.contains("| def main"), "{out}");
        assert!(out.contains("| import os"), "{out}");
    }

    #[test]
    fn javascript_outline_covers_functions_classes_arrows() {
        let src = "\
import { x } from './x';
export default class App {}
function helper() {}
const arrow = (a, b) => a + b;
export const named = async () => {};
let plain = 5;
";
        let out = outline(Language::JavaScript, src);
        assert!(out.contains("| class App"), "{out}");
        assert!(out.contains("| function helper"), "{out}");
        assert!(out.contains("| fn arrow"), "{out}");
        assert!(out.contains("| fn named"), "{out}");
        assert!(!out.contains("plain"), "{out}");
        assert!(out.contains("import { x }"), "{out}");
    }

    #[test]
    fn empty_text_yields_empty_outline() {
        assert_eq!(outline(Language::Rust, ""), "");
        assert_eq!(outline(Language::Python, "# nothing here\n"), "");
    }

    #[test]
    fn outline_caps_symbol_count() {
        let src: String = (0..300)
            .map(|i| format!("pub fn sym_{i}() {{}}\n"))
            .collect();
        let out = outline(Language::Rust, &src);
        assert!(out.contains("capped at 150 symbols"), "{out}");
    }
}
