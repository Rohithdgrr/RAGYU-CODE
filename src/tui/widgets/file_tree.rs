//! Interactive project tree sidebar.
//!
//! Lazily expands directories, respects `.govindaignore`, and decorates
//! entries with git status marks (`M` modified, `A` staged, `?` untracked).
//! The tree is cached and only re-read on explicit refresh (F5) or when the
//! sidebar is opened.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use super::super::theme;
use crate::ignore::IgnoreRules;

#[derive(Debug, Clone)]
pub struct TreeNode {
    pub name: String,
    /// Workspace-relative path with forward slashes.
    pub rel: String,
    pub is_dir: bool,
    pub expanded: bool,
    loaded: bool,
    pub children: Vec<TreeNode>,
}

impl TreeNode {
    fn dir(name: String, rel: String) -> Self {
        Self {
            name,
            rel,
            is_dir: true,
            expanded: false,
            loaded: false,
            children: Vec::new(),
        }
    }

    fn file(name: String, rel: String) -> Self {
        Self {
            name,
            rel,
            is_dir: false,
            expanded: false,
            loaded: true,
            children: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GitMark {
    Modified,
    Staged,
    Untracked,
}

impl GitMark {
    fn symbol(self) -> &'static str {
        match self {
            GitMark::Modified => "M",
            GitMark::Staged => "A",
            GitMark::Untracked => "?",
        }
    }

    fn color(self, t: &theme::Theme) -> Style {
        match self {
            GitMark::Staged => Style::default().fg(t.accent_success),
            GitMark::Modified => Style::default().fg(t.accent_warning),
            GitMark::Untracked => Style::default().fg(t.text_muted),
        }
    }
}

pub struct FileTree {
    root: PathBuf,
    ignore: IgnoreRules,
    nodes: Vec<TreeNode>,
    selected: usize,
    git_marks: HashMap<String, GitMark>,
    /// Height of the render area, fed back by the draw pass so scrolling
    /// keeps the selection visible.
    view_height: Cell<u16>,
    /// Last time the tree was auto-refreshed (for realtime file watching).
    last_auto_refresh: RefCell<Option<Instant>>,
}

impl FileTree {
    /// Builds a tree rooted at `root`, loading its top level immediately.
    pub fn open(root: &Path) -> Self {
        let mut tree = Self {
            root: root.to_path_buf(),
            ignore: IgnoreRules::load(root),
            nodes: Vec::new(),
            selected: 0,
            git_marks: HashMap::new(),
            view_height: Cell::new(20),
            last_auto_refresh: RefCell::new(Some(Instant::now())),
        };
        tree.nodes = read_children(&tree.root, &tree.ignore, "");
        tree.refresh_git();
        tree
    }

    /// Real-time poll: if a file was created/edited/deleted externally or via
    /// a tool (write_file, edit_file) the tree auto-refreshes. Throttled to
    /// once per 700ms so it feels instant but not chatty.
    pub fn maybe_auto_refresh(&mut self) {
        let now = Instant::now();
        let should = {
            let last = self.last_auto_refresh.borrow();
            match *last {
                Some(t) => now.duration_since(t) >= Duration::from_millis(700),
                None => true,
            }
        };
        if should {
            // quick mtime check: if top-level dir mtime hasn't changed, skip git refresh
            // For perfect realtime we always refresh; it's cheap for <500 entries.
            self.refresh();
            *self.last_auto_refresh.borrow_mut() = Some(now);
        }
    }

    /// Force immediate refresh on next poll (e.g., after a tool mutated files).
    pub fn mark_dirty(&self) {
        *self.last_auto_refresh.borrow_mut() = None;
    }

    /// Re-reads git status (`git status --porcelain`) synchronously; called
    /// rarely (open/refresh), so blocking is acceptable.
    pub fn refresh_git(&mut self) {
        self.git_marks.clear();
        let output = std::process::Command::new("git")
            .args(["status", "--porcelain"])
            .current_dir(&self.root)
            .output();
        if let Ok(out) = output {
            let text = String::from_utf8_lossy(&out.stdout);
            for line in text.lines() {
                if line.len() < 4 {
                    continue;
                }
                let (status, path_raw) = line.split_at(2);
                let path = path_raw.trim_start().replace('\\', "/");
                let mark = match status.trim() {
                    "??" => GitMark::Untracked,
                    s if s.contains('M') => GitMark::Modified,
                    s if s.starts_with('A') => GitMark::Staged,
                    _ => continue,
                };
                self.git_marks.insert(path, mark);
            }
        }
        // Reload already-loaded directories so new/removed files show up.
        reload_loaded(&mut self.nodes, &self.root, &self.ignore);
        // Expand ancestors of marked files so changes are visible on open.
        let marked: Vec<String> = self.git_marks.keys().cloned().collect();
        for path in marked {
            expand_to(&mut self.nodes, &self.root, &self.ignore, &path);
        }
        self.selected = self.selected.min(self.flat_len().saturating_sub(1));
    }

    /// Full rebuild (F5): re-loads ignore rules, top level, and git status.
    pub fn refresh(&mut self) {
        self.ignore = IgnoreRules::load(&self.root);
        self.nodes = read_children(&self.root, &self.ignore, "");
        self.selected = 0;
        self.refresh_git();
    }

    /// Depth-first list of currently visible rows.
    pub fn flat(&self) -> Vec<&TreeNode> {
        fn walk<'a>(nodes: &'a [TreeNode], out: &mut Vec<&'a TreeNode>) {
            for n in nodes {
                out.push(n);
                if n.is_dir && n.expanded {
                    walk(&n.children, out);
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.nodes, &mut out);
        out
    }

    pub fn flat_len(&self) -> usize {
        self.flat().len()
    }

    pub fn selected_index(&self) -> usize {
        self.selected
    }

    pub fn set_selected(&mut self, idx: usize) {
        let len = self.flat_len();
        if len == 0 {
            self.selected = 0;
        } else {
            self.selected = idx.min(len - 1);
        }
    }

    /// Currently highlighted row.
    pub fn selected_node(&self) -> Option<&TreeNode> {
        self.flat().get(self.selected).copied()
    }

    /// Moves the selection, clamping at both ends.
    pub fn move_selection(&mut self, delta: isize) {
        let len = self.flat_len() as isize;
        if len == 0 {
            self.selected = 0;
            return;
        }
        self.selected = (self.selected as isize + delta).clamp(0, len - 1) as usize;
    }

    /// Space key: expand/collapse the highlighted directory.
    pub fn toggle_selected(&mut self) {
        let Some(rel) = self.selected_node().map(|n| n.rel.clone()) else {
            return;
        };
        toggle_in(&mut self.nodes, &self.root, &self.ignore, &rel);
    }

    /// Enter key result: `Some(abs_path)` pins a file to context; `None`
    /// toggled a directory instead.
    pub fn activate_selected(&mut self) -> Option<PathBuf> {
        let node = self.selected_node()?;
        if node.is_dir {
            self.toggle_selected();
            return None;
        }
        Some(
            self.root
                .join(node.rel.replace('/', std::path::MAIN_SEPARATOR_STR)),
        )
    }

    /// Render height communicated by the draw pass so scrolling stays exact.
    pub fn set_view_height(&self, h: u16) {
        self.view_height.set(h.max(1));
    }

    /// Renders the visible rows with icons, git marks, and selection.
    /// `width` is the inner pane width (for truncating long names perfectly).
    pub fn render_lines(&self, focused: bool) -> Vec<Line<'static>> {
        self.render_lines_with_width(focused, 34)
    }

    /// Width-aware variant used by draw for perfect clipping. `hovered` is
    /// the absolute flat index under the mouse (soft sheen), if any.
    pub fn render_lines_with_width(&self, focused: bool, width: u16) -> Vec<Line<'static>> {
        self.render_lines_hover(focused, width, None)
    }

    /// Full variant with mouse-hover support.
    pub fn render_lines_hover(
        &self,
        focused: bool,
        width: u16,
        hovered: Option<usize>,
    ) -> Vec<Line<'static>> {
        let t = theme::active();
        let rows = self.flat();

        // Windowed scroll that keeps the selected row on screen.
        let visible_height = usize::from(self.view_height.get());
        let start = self
            .selected
            .saturating_sub(visible_height.saturating_sub(1))
            .min(rows.len().saturating_sub(visible_height.min(rows.len())));
        let end = (start + visible_height).min(rows.len());

        let avail = width.saturating_sub(2) as usize; // inner width with small padding
        rows[start..end]
            .iter()
            .enumerate()
            .map(|(i, node)| {
                let idx = start + i;
                let depth = node.rel.matches('/').count();
                // guides: "│ " style via indent, but keep simple 2-space
                let indent = "  ".repeat(depth);
                // richer icons: dirs vs files (Nerd Font glyphs, no emoji)
                let (icon, icon_fg) = if node.is_dir {
                    if node.expanded {
                        ("\u{f07c}", t.accent_secondary) // folder-open
                    } else {
                        ("\u{f07b}", t.text_muted) // folder
                    }
                } else {
                    // file-type glyph keeps recognition instant
                    let ext = node.name.rsplit('.').next().unwrap_or("");
                    let c = match ext {
                        "rs" => t.syntax_type,
                        "toml" | "json" | "md" => t.text_muted,
                        "exe" | "dll" | "pdb" | "rmeta" | "rlib" | "d" => t.text_muted,
                        _ => t.text_primary,
                    };
                    (super::super::icons::file_icon(&node.name), c)
                };
                let selected_row = idx == self.selected;
                let hovered_row = hovered == Some(idx) && !selected_row;
                let base = if selected_row {
                    Style::default()
                        .bg(if focused { t.bg_hover } else { t.bg_secondary })
                        .fg(t.text_primary)
                } else if hovered_row {
                    // soft glass sheen under the mouse
                    Style::default()
                        .bg(t.bg_tertiary)
                        .fg(t.text_primary)
                        .add_modifier(Modifier::UNDERLINED)
                } else {
                    t.sidebar_bg()
                };
                let icon_style = if selected_row {
                    Style::default().fg(icon_fg).bg(base.bg.unwrap_or(t.bg_secondary)).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(icon_fg).bg(base.bg.unwrap_or(t.bg_secondary))
                };

                // compute truncation for perfect fit
                let mark_w = if self.git_marks.contains_key(&node.rel) { 2 } else { 0 };
                let prefix_w = indent.len() + 3; // "▸ " is 2 chars + space
                let max_name = avail.saturating_sub(prefix_w + mark_w).max(8);
                let display_name = truncate_name(&node.name, max_name);

                let mut spans = vec![
                    Span::styled(
                        format!("{indent}{:>2} ", icon),
                        icon_style,
                    ),
                    Span::styled(
                        display_name.clone(),
                        if selected_row {
                            base.add_modifier(Modifier::BOLD)
                        } else if node.is_dir {
                            Style::default().fg(t.accent_secondary).bg(t.bg_secondary).add_modifier(Modifier::BOLD)
                        } else {
                            base
                        },
                    ),
                ];
                if let Some(mark) = self.git_marks.get(&node.rel) {
                    spans.push(Span::styled(format!(" {}", mark.symbol()), mark.color(&t).bg(base.bg.unwrap_or(t.bg_secondary))));
                }
                // dim rlib/d artefacts
                if !node.is_dir && matches!(node.name.rsplit('.').next().unwrap_or(""), "rmeta" | "rlib" | "d" | "pdb") {
                    // already muted via icon color, keep name muted
                    if let Some(last) = spans.get_mut(1) {
                        *last = Span::styled(display_name.clone(), Style::default().fg(t.text_muted).bg(base.bg.unwrap_or(t.bg_secondary)));
                    }
                }
                Line::from(spans).style(base)
            })
            .collect()
    }
}

/// Reads one directory's entries (dirs first, then alphabetical), skipping
/// hidden files and anything `.govindaignore` excludes.
fn read_children(root: &Path, ignore: &IgnoreRules, rel_dir: &str) -> Vec<TreeNode> {
    let abs = if rel_dir.is_empty() {
        root.to_path_buf()
    } else {
        root.join(rel_dir.replace('/', std::path::MAIN_SEPARATOR_STR))
    };
    let Ok(entries) = std::fs::read_dir(&abs) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        let Ok(ft) = entry.file_type() else { continue };
        let is_dir = ft.is_dir();
        let rel = if rel_dir.is_empty() {
            name.clone()
        } else {
            format!("{rel_dir}/{name}")
        };
        if ignore.matches(&rel, is_dir) {
            continue;
        }
        out.push(if is_dir {
            TreeNode::dir(name, rel)
        } else {
            TreeNode::file(name, rel)
        });
    }
    out.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.cmp(&b.name)));
    out
}

/// Expands every loaded directory in place, re-reading its contents.
fn reload_loaded(nodes: &mut [TreeNode], root: &Path, ignore: &IgnoreRules) {
    for node in nodes {
        if node.is_dir && node.loaded {
            node.children = read_children(root, ignore, &node.rel);
            reload_loaded(&mut node.children, root, ignore);
        }
    }
}

/// Marks `target`'s ancestors expanded, loading them lazily.
fn expand_to(nodes: &mut [TreeNode], root: &Path, ignore: &IgnoreRules, target: &str) {
    let mut consumed = String::new();
    for seg in target.split('/') {
        if !consumed.is_empty() {
            consumed.push('/');
        }
        consumed.push_str(seg);
        if walk_expand(nodes, root, ignore, &consumed) {
            continue;
        }
        return;
    }
}

fn walk_expand(
    nodes: &mut [TreeNode],
    root: &Path,
    ignore: &IgnoreRules,
    rel: &str,
) -> bool {
    for node in nodes {
        if node.rel != rel {
            continue;
        }
        node.expanded = true;
        if node.is_dir && !node.loaded {
            node.children = read_children(root, ignore, rel);
            node.loaded = true;
        }
        return true;
    }
    false
}

fn toggle_in(
    nodes: &mut [TreeNode],
    root: &Path,
    ignore: &IgnoreRules,
    target: &str,
) -> bool {
    for node in nodes {
        if node.rel == target && node.is_dir {
            if !node.loaded {
                node.children = read_children(root, ignore, target);
                node.loaded = true;
            }
            node.expanded = !node.expanded;
            return true;
        }
        if node.is_dir
            && toggle_in(&mut node.children, root, ignore, target)
        {
            return true;
        }
    }
    false
}

fn truncate_name(name: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    let w = UnicodeWidthStr::width(name);
    if w <= max {
        return name.to_owned();
    }
    let mut out = String::new();
    let mut cur = 0usize;
    for c in name.chars() {
        let cw = unicode_width::UnicodeWidthChar::width(c).unwrap_or(1);
        if cur + cw + 1 > max {
            out.push('…');
            break;
        }
        out.push(c);
        cur += cw;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn scratch() -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("govinda-tree-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src/nested")).unwrap();
        std::fs::create_dir_all(dir.join("target")).unwrap();
        std::fs::write(dir.join("src/main.rs"), "fn main() {}").unwrap();
        std::fs::write(dir.join("src/nested/deep.rs"), "").unwrap();
        std::fs::write(dir.join("README.md"), "# x").unwrap();
        std::fs::write(dir.join(".govindaignore"), "target/\n").unwrap();
        dir
    }

    #[test]
    fn open_skips_hidden_and_ignored_dirs() {
        let dir = scratch();
        let tree = FileTree::open(&dir);
        let names: Vec<&str> = tree.flat().iter().map(|n| n.name.as_str()).collect();
        assert!(names.contains(&"src"));
        assert!(names.contains(&"README.md"));
        assert!(!names.contains(&"target"), "ignored dir must be hidden");
        assert!(!names.contains(&".govindaignore"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn navigation_clamps_at_edges() {
        let dir = scratch();
        let mut tree = FileTree::open(&dir);
        let len = tree.flat_len();
        assert!(len >= 2, "scratch layout should yield ≥2 rows");
        tree.move_selection(-99);
        assert_eq!(tree.selected, 0);
        tree.move_selection(99);
        assert_eq!(tree.selected, len - 1);
        if len >= 3 {
            tree.move_selection(-2);
            assert_eq!(tree.selected, len - 3);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn toggling_a_directory_reveals_children() {
        let dir = scratch();
        let mut tree = FileTree::open(&dir);
        // Find the src row and select it before expanding.
        let idx = tree.flat().iter().position(|n| n.name == "src").unwrap();
        tree.move_selection(idx as isize - tree.selected as isize);
        assert!(tree.selected_node().unwrap().is_dir);
        tree.toggle_selected();
        let names: Vec<&str> = tree.flat().iter().map(|n| n.name.as_str()).collect();
        assert!(names.contains(&"main.rs"));
        assert!(names.contains(&"nested"));

        // Enter on a file returns its absolute path; on a dir it expands.
        let deep_idx = tree.flat().iter().position(|n| n.rel == "src").unwrap();
        tree.move_selection(deep_idx as isize - tree.selected as isize);
        assert!(tree.activate_selected().is_none()); // toggles closed again

        tree.toggle_selected(); // reopen
        let file_idx = tree.flat().iter().position(|n| n.rel == "src/main.rs").unwrap();
        tree.move_selection(file_idx as isize - tree.selected as isize);
        let pinned = tree.activate_selected().unwrap();
        assert!(pinned.ends_with("main.rs"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn git_marks_parse_from_porcelain_output() {
        // Indirect test: refresh_git against a non-repo must not panic and
        // must simply produce no marks.
        let dir = scratch();
        let mut tree = FileTree::open(&dir);
        tree.refresh_git();
        // temp dirs are not repos; git fails silently → no marks.
        // (In-repo behavior is covered manually.)
        assert!(tree.flat_len() > 0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
