//! `git_tools` — extended git operations: stash, branch create, checkout,
//! rebase, merge, remote add, tag.

use std::path::Path;

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "snake_case", tag = "action")]
pub enum GitAction {
    Stash {
        sub: Option<String>,
        message: Option<String>,
    },
    CreateBranch {
        name: String,
        from: Option<String>,
    },
    Checkout {
        name: String,
    },
    Rebase {
        upstream: String,
        interactive: Option<bool>,
    },
    Merge {
        branch: String,
        no_ff: Option<bool>,
    },
    RemoteAdd {
        name: String,
        url: String,
    },
    Tag {
        name: String,
        sha: Option<String>,
        message: Option<String>,
    },
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Args {
    pub action: GitAction,
}

pub async fn run(base: &Path, args: Args) -> anyhow::Result<String> {
    let argv: Vec<String> = match &args.action {
        GitAction::Stash { sub, message } => {
            let mut a = vec!["stash".to_string()];
            if let Some(s) = sub {
                a.push(s.clone());
            }
            if let Some(m) = message {
                a.push("-m".to_string());
                a.push(m.clone());
            }
            a
        }
        GitAction::CreateBranch { name, from } => {
            let mut a = vec!["checkout".to_string(), "-b".to_string(), name.clone()];
            if let Some(f) = from {
                a.push(f.clone());
            }
            a
        }
        GitAction::Checkout { name } => vec!["checkout".to_string(), name.clone()],
        GitAction::Rebase {
            upstream,
            interactive,
        } => {
            let mut a = vec!["rebase".to_string()];
            if interactive.unwrap_or(false) {
                a.push("-i".to_string());
            }
            a.push(upstream.clone());
            a
        }
        GitAction::Merge { branch, no_ff } => {
            let mut a = vec!["merge".to_string()];
            if no_ff.unwrap_or(false) {
                a.push("--no-ff".to_string());
            }
            a.push(branch.clone());
            a
        }
        GitAction::RemoteAdd { name, url } => {
            vec![
                "remote".to_string(),
                "add".to_string(),
                name.clone(),
                url.clone(),
            ]
        }
        GitAction::Tag { name, sha, message } => {
            let mut a = vec!["tag".to_string()];
            if let Some(m) = message {
                a.push("-m".to_string());
                a.push(m.clone());
            }
            a.push(name.clone());
            if let Some(s) = sha {
                a.push(s.clone());
            }
            a
        }
    };
    let output = std::process::Command::new("git")
        .args(&argv)
        .current_dir(base)
        .output()
        .map_err(|e| anyhow::anyhow!("failed to spawn git: {e}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let ok = output.status.success();
    Ok(format!(
        "{{\"ok\":{ok},\"argv\":{},\"stdout\":{},\"stderr\":{}}}",
        serde_json::to_string(&argv).unwrap_or_default(),
        serde_json::Value::String(truncate(&stdout, 2000)),
        serde_json::Value::String(truncate(&stderr, 2000)),
    ))
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_owned()
    } else {
        let mut out: String = s.chars().take(max).collect();
        out.push('…');
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_stash_argv() {
        let action = GitAction::Stash {
            sub: Some("push".into()),
            message: Some("WIP".into()),
        };
        let argv = match action {
            GitAction::Stash { sub, message } => {
                let mut a = vec!["stash".to_string()];
                if let Some(s) = sub {
                    a.push(s.clone());
                }
                if let Some(m) = message {
                    a.push("-m".to_string());
                    a.push(m.clone());
                }
                a
            }
            _ => unreachable!(),
        };
        assert_eq!(argv, vec!["stash", "push", "-m", "WIP"]);
    }
}
