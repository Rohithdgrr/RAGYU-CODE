//! Skills / Slash-Command Extensions
//!
//! Loads custom slash commands from `~/.config/govinda/skills/*.md` files.
//! Each `.md` file defines a skill with a frontmatter header and body.
//!
//! Format:
//! ```markdown
//! ---
//! name: /my-skill
//! description: Does something useful
//! args: optional|required
//! ---
//! This is the skill prompt/instructions body.
//! When invoked, the body is sent as context to the model.
//! ```

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

/// A loaded skill definition from a `.md` file.
#[derive(Debug, Clone)]
pub struct Skill {
    /// The slash command name (e.g., `/my-skill`).
    pub name: String,
    /// Human-readable description.
    pub description: String,
    /// Whether the skill requires arguments.
    pub requires_args: bool,
    /// The skill body/prompt text.
    pub body: String,
    /// Source file path (for diagnostics).
    pub source: PathBuf,
}

/// Returns the skills directory path (`~/.config/govinda/skills/`).
pub fn skills_dir() -> Option<PathBuf> {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .ok()
        .map(PathBuf::from)?;
    Some(home.join(".config").join("govinda").join("skills"))
}

/// Loads all skills from the skills directory.
pub fn load_skills() -> Vec<Skill> {
    let Some(dir) = skills_dir() else {
        return Vec::new();
    };
    if !dir.is_dir() {
        return Vec::new();
    }
    let mut skills = Vec::new();
    let Ok(entries) = fs::read_dir(&dir) else {
        return Vec::new();
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "md") {
            match parse_skill_file(&path) {
                Ok(skill) => skills.push(skill),
                Err(e) => {
                    eprintln!("warning: failed to load skill {}: {e:#}", path.display());
                }
            }
        }
    }
    skills.sort_by(|a, b| a.name.cmp(&b.name));
    skills
}

/// Parses a single skill `.md` file.
fn parse_skill_file(path: &Path) -> Result<Skill> {
    let raw =
        fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;

    let raw = raw.trim();
    anyhow::ensure!(!raw.is_empty(), "empty skill file");

    // Parse YAML-like frontmatter between `---` markers.
    let (frontmatter, body) = if let Some(rest) = raw.strip_prefix("---") {
        let Some(end) = rest.find("---") else {
            anyhow::bail!("unclosed frontmatter in {}", path.display());
        };
        let fm_str = rest[..end].trim();
        let body_str = rest[end + 3..].trim();
        (fm_str, body_str)
    } else {
        // No frontmatter: use filename as name, whole file as body.
        let name = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "unknown".into());
        return Ok(Skill {
            name: format!("/{name}"),
            description: format!("Custom skill from {}", path.display()),
            requires_args: false,
            body: raw.to_owned(),
            source: path.to_path_buf(),
        });
    };

    let mut meta: HashMap<&str, &str> = HashMap::new();
    for line in frontmatter.lines() {
        if let Some((key, val)) = line.split_once(':') {
            meta.insert(key.trim(), val.trim());
        }
    }

    let name = meta.get("name").map(|s| s.to_string()).unwrap_or_else(|| {
        path.file_stem()
            .map(|s| format!("/{}", s.to_string_lossy()))
            .unwrap_or_else(|| "/unknown".into())
    });

    let description = meta
        .get("description")
        .unwrap_or(&"Custom skill")
        .to_string();

    let requires_args = meta.get("args").map(|s| *s == "required").unwrap_or(false);

    anyhow::ensure!(!body.is_empty(), "skill body is empty");

    Ok(Skill {
        name,
        description,
        requires_args,
        body: body.to_owned(),
        source: path.to_path_buf(),
    })
}

/// Builds a skill map from command name to skill for quick lookup.
pub fn skill_map(skills: &[Skill]) -> HashMap<String, &Skill> {
    skills.iter().map(|s| (s.name.clone(), s)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_skill_with_frontmatter() {
        let dir = std::env::temp_dir().join("govinda-skill-test");
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("test-skill.md");
        fs::write(
            &path,
            "---\nname: /test-skill\ndescription: A test skill\nargs: required\n---\nSkill body here.\n",
        )
        .unwrap();
        let skill = parse_skill_file(&path).unwrap();
        assert_eq!(skill.name, "/test-skill");
        assert_eq!(skill.description, "A test skill");
        assert!(skill.requires_args);
        assert_eq!(skill.body, "Skill body here.");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn parse_skill_without_frontmatter() {
        let dir = std::env::temp_dir().join("govinda-skill-test2");
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("plain.md");
        fs::write(&path, "Just a plain body.\n").unwrap();
        let skill = parse_skill_file(&path).unwrap();
        assert_eq!(skill.name, "/plain");
        assert!(!skill.requires_args);
        assert_eq!(skill.body, "Just a plain body.");
        let _ = fs::remove_dir_all(&dir);
    }
}
