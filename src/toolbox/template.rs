//! `template` — project scaffold generator.
//!
//! Produces a complete, runnable project structure for web, app, bot,
//! extension, AI/ML, CLI, and TUI projects. One call replaces 20+ tool
//! calls to lay down boilerplate.

use std::collections::BTreeMap;

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    Web,
    App,
    Bot,
    Extension,
    AiMl,
    Cli,
    Tui,
    Api,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Args {
    pub kind: Kind,
    /// Framework name (nextjs, vite, tauri, electron, discord.py, ratatui, ...).
    /// Defaults to a sensible pick for the kind.
    pub framework: Option<String>,
    /// Project name (used for directory, package name, etc.).
    pub name: String,
    /// Workspace-relative path where the project is created.
    pub path: String,
}

pub fn run(base: &std::path::Path, args: Args) -> anyhow::Result<String> {
    let root = base.join(&args.path);
    anyhow::ensure!(
        !root.exists(),
        "destination already exists: {}",
        root.display()
    );
    std::fs::create_dir_all(&root)?;

    let files: BTreeMap<&str, String> = match (&args.kind, args.framework.as_deref()) {
        (Kind::Cli, Some("rust") | None) => scaffold_rust_cli(&args.name),
        (Kind::Cli, Some("go")) => scaffold_go_cli(&args.name),
        (Kind::Cli, Some("node")) => scaffold_node_cli(&args.name),
        (Kind::Web, Some("nextjs") | None) => scaffold_nextjs(&args.name),
        (Kind::Web, Some("vite")) => scaffold_vite(&args.name),
        (Kind::App, Some("tauri")) => scaffold_tauri(&args.name),
        (Kind::Bot, Some("discord")) => scaffold_discord_bot(&args.name),
        (Kind::Tui, Some("ratatui") | None) => scaffold_ratatui(&args.name),
        (Kind::Tui, Some("bubbletea")) => scaffold_bubbletea(&args.name),
        (Kind::Api, Some("axum") | None) => scaffold_axum(&args.name),
        (Kind::AiMl, Some("pytorch")) => scaffold_pytorch(&args.name),
        (Kind::Extension, Some("chrome")) => scaffold_chrome_ext(&args.name),
        _ => anyhow::bail!(
            "unsupported kind/framework combination: {:?}/{:?}",
            args.kind,
            args.framework
        ),
    };

    let mut written: Vec<String> = Vec::new();
    for (rel, content) in &files {
        let full = root.join(rel);
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&full, content)?;
        written.push(rel.to_string());
    }
    Ok(format!(
        "{{\"name\":\"{}\",\"kind\":\"{:?}\",\"framework\":\"{}\",\"files\":{},\"path\":\"{}\"}}",
        args.name,
        args.kind,
        args.framework.as_deref().unwrap_or("(default)"),
        written.len(),
        root.display()
    ))
}

fn scaffold_rust_cli(name: &str) -> BTreeMap<&'static str, String> {
    let mut m = BTreeMap::new();
    m.insert("Cargo.toml", rust_cli_cargo_toml(name));
    m.insert("src/main.rs", rust_cli_main_rs(name));
    m.insert(
        "README.md",
        format!("# {name}\n\nA Rust CLI built with the GOVINDA scaffold.\n\n## Build\n\n```\ncargo build --release\n```\n"),
    );
    m.insert(".gitignore", "/target\nCargo.lock\n".to_string());
    m
}

fn rust_cli_cargo_toml(name: &str) -> String {
    format!(
        "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nclap = {{ version = \"4\", features = [\"derive\"] }}\nanyhow = \"1\"\n\n[[bin]]\nname = \"{name}\"\npath = \"src/main.rs\"\n"
    )
}

fn rust_cli_main_rs(name: &str) -> String {
    format!(
        "use clap::Parser;\n\n#[derive(Parser, Debug)]\n#[command(name = \"{name}\", about = \"CLI built with govinda scaffold\")]\nstruct Args {{\n    /// Name to greet\n    #[arg(short, long, default_value = \"world\")]\n    name: String,\n}}\n\nfn main() -> anyhow::Result<()> {{\n    let args = Args::parse();\n    println!(\"Hello, {{}}!\", args.name);\n    Ok(())\n}}\n"
    )
}

fn scaffold_go_cli(name: &str) -> BTreeMap<&'static str, String> {
    let mut m = BTreeMap::new();
    m.insert("go.mod", format!("module {name}\n\ngo 1.22\n"));
    m.insert(
        "main.go",
        format!(
            "package main\n\nimport \"fmt\"\n\nfunc main() {{\n\tfmt.Println(\"Hello, world! ({name})\")\n}}\n"
        ),
    );
    m
}

fn scaffold_node_cli(name: &str) -> BTreeMap<&'static str, String> {
    let mut m = BTreeMap::new();
    m.insert(
        "package.json",
        format!(
            "{{\n  \"name\": \"{name}\",\n  \"version\": \"0.1.0\",\n  \"type\": \"module\",\n  \"bin\": \"{{\\\"{name}\\\": \\\"./bin.mjs\\\"}}\",\n  \"dependencies\": {{\"commander\": \"^12\"}}\n}}\n"
        ),
    );
    m.insert(
        "bin.mjs",
        "#!/usr/bin/env node\nimport { program } from 'commander';\nprogram.name('".to_string()
            + name
            + "').description('CLI built with govinda scaffold').version('0.1.0')\n  .command('hello <name>').action((name) => console.log(`Hello, ${name}!`))\n  .parse();\n",
    );
    m
}

fn scaffold_nextjs(name: &str) -> BTreeMap<&'static str, String> {
    let mut m = BTreeMap::new();
    m.insert(
        "package.json",
        format!(
            "{{\n  \"name\": \"{name}\",\n  \"version\": \"0.1.0\",\n  \"private\": true,\n  \"scripts\": {{\"dev\": \"next dev\", \"build\": \"next build\", \"start\": \"next start\"}}\n}}\n"
        ),
    );
    m.insert("next.config.mjs", "/** @type {{import('next').NextConfig}} */\nconst nextConfig = {{}};\nexport default nextConfig;\n".to_string());
    m.insert(
        "app/page.tsx",
        "export default function Home() { return <main><h1>".to_string()
            + name
            + "</h1><p>Built with govinda scaffold.</p></main>; }\n",
    );
    m.insert(
        "app/layout.tsx",
        "import './globals.css';\nexport default function RootLayout({ children }: { children: React.ReactNode }) { return <html><body>{children}</body></html>; }\n".to_string(),
    );
    m.insert(
        "app/globals.css",
        "html, body { padding: 0; margin: 0; font-family: system-ui, sans-serif; }\n".to_string(),
    );
    m.insert(
        "tsconfig.json",
        "{\n  \"compilerOptions\": {\n    \"target\": \"es2017\",\n    \"lib\": [\"dom\", \"dom.iterable\", \"esnext\"],\n    \"allowJs\": true,\n    \"skipLibCheck\": true,\n    \"strict\": true,\n    \"noEmit\": true,\n    \"esModuleInterop\": true,\n    \"module\": \"esnext\",\n    \"moduleResolution\": \"bundler\",\n    \"resolveJsonModule\": true,\n    \"isolatedModules\": true,\n    \"jsx\": \"preserve\",\n    \"incremental\": true,\n    \"plugins\": [{ \"name\": \"next\" }]\n  },\n  \"include\": [\"next-env.d.ts\", \"**/*.ts\", \"**/*.tsx\"],\n  \"exclude\": [\"node_modules\"]\n}\n".to_string(),
    );
    m
}

fn scaffold_vite(name: &str) -> BTreeMap<&'static str, String> {
    let mut m = BTreeMap::new();
    m.insert(
        "package.json",
        format!(
            "{{\n  \"name\": \"{name}\",\n  \"private\": true,\n  \"version\": \"0.1.0\",\n  \"type\": \"module\",\n  \"scripts\": {{\"dev\": \"vite\", \"build\": \"vite build\", \"preview\": \"vite preview\"}}\n}}\n"
        ),
    );
    m.insert(
        "index.html",
        format!(
            "<!doctype html>\n<html lang=\"en\">\n  <head><meta charset=\"UTF-8\" /><title>{name}</title></head>\n  <body><div id=\"app\"></div><script type=\"module\" src=\"/src/main.ts\"></script></body>\n</html>\n"
        ),
    );
    m.insert("src/main.ts", format!("import './style.css';\ndocument.querySelector<HTMLDivElement>('#app')!.innerHTML = `<h1>{name}</h1>`;\n"));
    m.insert(
        "src/style.css",
        "body { font-family: system-ui; padding: 2rem; }\n".to_string(),
    );
    m.insert(
        "vite.config.ts",
        "import { defineConfig } from 'vite';\nexport default defineConfig({});\n".to_string(),
    );
    m.insert("tsconfig.json", "{ \"compilerOptions\": { \"target\": \"es2020\", \"useDefineForClassFields\": true, \"module\": \"esnext\", \"moduleResolution\": \"bundler\", \"strict\": true, \"jsx\": \"preserve\" }, \"include\": [\"src\"] }\n".to_string());
    m
}

fn scaffold_tauri(name: &str) -> BTreeMap<&'static str, String> {
    let mut m = BTreeMap::new();
    m.insert("src-tauri/Cargo.toml", format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[lib]\nname = \"lib_{name}\"\ncrate-type = [\"staticlib\", \"cdylib\", \"rlib\"]\n\n[dependencies]\ntauri = {{ version = \"1\", features = [] }}\n"));
    m.insert(
        "src-tauri/src/main.rs",
        "fn main() { tauri::Builder::default().run(tauri::generate_context!()).expect(\"error\"); }\n".to_string(),
    );
    m.insert(
        "package.json",
        format!(
            "{{\"name\": \"{name}\", \"private\": true, \"scripts\": {{\"tauri\": \"tauri\"}}}}\n"
        ),
    );
    m
}

fn scaffold_discord_bot(name: &str) -> BTreeMap<&'static str, String> {
    let mut m = BTreeMap::new();
    m.insert(
        "bot.py",
        format!(
            "import discord\nfrom discord.ext import commands\n\nbot = commands.Bot(command_prefix='!')\n\n@bot.event\nasync def on_ready():\n    print(f'{{bot.user}} ready ({name})')\n\n@bot.command()\nasync def hello(ctx):\n    await ctx.send('Hello, {{}}!'.format(ctx.author.mention))\n\nbot.run('YOUR_TOKEN_HERE')\n"
        ),
    );
    m.insert(
        "requirements.txt",
        "discord.py>=2.3\npython-dotenv>=1.0\n".to_string(),
    );
    m.insert(".env.example", "DISCORD_TOKEN=\n".to_string());
    m.insert(
        "README.md",
        format!(
            "# {name}\n\nA Discord bot scaffold.\n\n## Setup\n\n1. Create an app at https://discord.com/developers\n2. Copy the bot token\n3. `cp .env.example .env` and add your token\n4. `pip install -r requirements.txt`\n5. `python bot.py`\n"
        ),
    );
    m
}

fn scaffold_ratatui(name: &str) -> BTreeMap<&'static str, String> {
    let mut m = BTreeMap::new();
    m.insert("Cargo.toml", format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nratatui = \"0.29\"\ncrossterm = \"0.28\"\nanyhow = \"1\"\n"));
    m.insert(
        "src/main.rs",
        "use anyhow::Result;\nuse crossterm::event::{self, Event, KeyCode};\nuse ratatui::{Frame, Terminal, backend::CrosstermBackend, widgets::{Block, Paragraph}};\n\nfn main() -> Result<()> {\n    let mut terminal = Terminal::new(CrosstermBackend::new(std::io::stderr()))?;\n    crossterm::terminal::enable_raw_mode()?;\n    loop {\n        terminal.draw(|f| ui(f))?;\n        if let Event::Key(k) = event::read()? {\n            if k.code == KeyCode::Char('q') { break; }\n        }\n    }\n    crossterm::terminal::disable_raw_mode()?;\n    Ok(())\n}\n\nfn ui(f: &mut Frame) {\n    let block = Block::default().title(\"GOVINDA TUI\");\n    f.render_widget(Paragraph::new(\"Press q to quit\").block(block), f.size());\n}\n".to_string(),
    );
    m
}

fn scaffold_bubbletea(name: &str) -> BTreeMap<&'static str, String> {
    let mut m = BTreeMap::new();
    m.insert(
        "go.mod",
        format!("module {name}\n\ngo 1.22\n\nrequire github.com/charmbracelet/bubbletea v0.26\n"),
    );
    m.insert(
        "main.go",
        "package main\n\nimport (\n\t\"fmt\"\n\ttea \"github.com/charmbracelet/bubbletea\"\n)\n\ntype model struct{}\n\nfunc (m model) Init() tea.Cmd { return nil }\n\nfunc (m model) Update(msg tea.Msg) (tea.Model, tea.Cmd) {\n\tswitch msg := msg.(type) {\n\tcase tea.KeyMsg:\n\t\tif msg.String() == \"q\" { return m, tea.Quit }\n\t}\n\treturn m, nil\n}\n\nfunc (m model) View() string { return \"GOVINDA TUI — press q to quit\\n\" }\n\nfunc main() {\n\tp := tea.NewProgram(model{})\n\tif _, err := p.Run(); err != nil { fmt.Println(err); }\n}\n".to_string(),
    );
    m
}

fn scaffold_axum(name: &str) -> BTreeMap<&'static str, String> {
    let mut m = BTreeMap::new();
    m.insert("Cargo.toml", format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\naxum = \"0.7\"\ntokio = {{ version = \"1\", features = [\"full\"] }}\ntower = \"0.5\"\n"));
    m.insert("src/main.rs", "use axum::{routing::get, Router};\n\n#[tokio::main]\nasync fn main() {\n    let app = Router::new().route(\"/\", get(|| async { \"Hello, world!\" }));\n    let listener = tokio::net::TcpListener::bind(\"0.0.0.0:3000\").await.unwrap();\n    axum::serve(listener, app).await.unwrap();\n}\n".to_string());
    m
}

fn scaffold_pytorch(name: &str) -> BTreeMap<&'static str, String> {
    let mut m = BTreeMap::new();
    m.insert(
        "train.py",
        format!(
            "import torch\nfrom torch import nn\nfrom torch.utils.data import DataLoader\nfrom torchvision import datasets, transforms\n\nclass Net(nn.Module):\n    def __init__(self):\n        super().__init__()\n        self.fc = nn.Linear(784, 10)\n    def forward(self, x): return self.fc(x.view(x.size(0), -1))\n\ndef main():\n    device = 'cuda' if torch.cuda.is_available() else 'cpu'\n    ds = datasets.MNIST('.', train=True, download=True, transform=transforms.ToTensor())\n    loader = DataLoader(ds, batch_size=64, shuffle=True)\n    model = Net().to(device)\n    opt = torch.optim.Adam(model.parameters())\n    for epoch in range(1):\n        for x, y in loader:\n            x, y = x.to(device), y.to(device)\n            opt.zero_grad()\n            loss = nn.functional.cross_entropy(model(x), y)\n            loss.backward()\n            opt.step()\n    print(f'{name} done')\n\nif __name__ == '__main__': main()\n"
        ),
    );
    m.insert(
        "requirements.txt",
        "torch>=2.0\ntorchvision>=0.15\n".to_string(),
    );
    m.insert(
        "README.md",
        format!(
            "# {name}\n\nPyTorch training scaffold.\n\n```\npip install -r requirements.txt\npython train.py\n```\n"
        ),
    );
    m
}

fn scaffold_chrome_ext(name: &str) -> BTreeMap<&'static str, String> {
    let mut m = BTreeMap::new();
    m.insert(
        "manifest.json",
        "{\n  \"manifest_version\": 3,\n  \"name\": \"".to_string()
            + name
            + "\",\n  \"version\": \"0.1.0\",\n  \"action\": {\"default_popup\": \"popup.html\"}\n}\n",
    );
    m.insert("popup.html", format!("<!doctype html>\n<html><head><title>{name}</title></head><body><h1>{name}</h1></body></html>\n"));
    m.insert(
        "popup.js",
        format!("document.body.textContent = '{name} loaded';\n"),
    );
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_cli_scaffold_has_required_files() {
        let files = scaffold_rust_cli("mycli");
        assert!(files.contains_key("Cargo.toml"));
        assert!(files.contains_key("src/main.rs"));
        assert!(files.contains_key("README.md"));
        assert!(files.contains_key(".gitignore"));
        assert!(files["Cargo.toml"].contains("clap"));
    }

    #[test]
    fn ratatui_scaffold_imports_correctly() {
        let files = scaffold_ratatui("mytui");
        let main = &files["src/main.rs"];
        assert!(main.contains("ratatui"));
        assert!(main.contains("crossterm"));
    }

    #[test]
    fn axum_scaffold_has_server() {
        let files = scaffold_axum("myserver");
        let main = &files["src/main.rs"];
        assert!(main.contains("axum::serve"));
        assert!(main.contains("Router"));
    }
}
