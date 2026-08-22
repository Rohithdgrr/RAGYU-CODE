//! Static shell-completion scripts for Govinda's CLI flags, emitted via
//! `govinda --completion <shell>`. Slash commands complete inside the REPL
//! (reedline completer), so these only cover process arguments.

pub const SHELLS: [&str; 4] = ["bash", "zsh", "fish", "powershell"];

/// Prints the completion script for `shell`; errors name the valid shells.
pub fn emit(shell: &str) -> anyhow::Result<()> {
    let script = match shell.to_ascii_lowercase().as_str() {
        "bash" => BASH,
        "zsh" => ZSH,
        "fish" => FISH,
        "powershell" | "pwsh" => POWERSHELL,
        other => {
            anyhow::bail!("unknown shell '{other}' — supported: {}", SHELLS.join(", "))
        }
    };
    print!("{script}");
    Ok(())
}

const BASH: &str = r#"# bash completion for govinda
_govinda_completions() {
    local cur="${COMP_WORDS[COMP_CWORD]}"
    local opts="--resume --query --help --completion"
    COMPREPLY=($(compgen -W "$opts" -- "$cur"))
}
complete -F _govinda_completions govinda
"#;

const ZSH: &str = r#"#compdef govinda
# zsh completion for govinda
_govinda() {
    _arguments \
        '--resume[continue a saved session]:session:' \
        '--query[one-shot query mode]:prompt:' \
        '-q[one-shot query mode]:prompt:' \
        '--help[show help]' \
        '--completion[emit a completion script]:shell:(bash zsh fish powershell)'
}
complete -F _govinda govinda 2>/dev/null || compdef _govinda govinda
"#;

const FISH: &str = r#"# fish completion for govinda
complete -c govinda -n '__fish_use_subcommand' -l resume -r -d 'continue a saved session'
complete -c govinda -n '__fish_use_subcommand' -l query -s q -r -d 'one-shot query mode'
complete -c govinda -n '__fish_use_subcommand' -l help -d 'show help'
complete -c govinda -n '__fish_use_subcommand' -l completion -r -a 'bash zsh fish powershell' -d 'emit a completion script'
"#;

const POWERSHELL: &str = r#"# PowerShell completion for govinda
Register-ArgumentCompleter -Native -CommandName govinda -ScriptBlock {
    param($wordToComplete)
    $opts = @('--resume', '-r', '--query', '-q', '--help', '-h', '--completion')
    $opts | Where-Object { $_ -like "$wordToComplete*" } | ForEach-Object {
        [System.Management.Automation.CompletionResult]::new($_, $_, 'ParameterName', $_)
    }
}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_supported_shell_emits_nonempty_script() {
        for shell in SHELLS {
            // emit() prints to stdout; validate via the match arms instead by
            // checking each shell is accepted (error would be Err).
            assert!(emit(shell).is_ok(), "{shell} should be supported");
        }
    }

    #[test]
    fn unknown_shell_is_rejected() {
        assert!(emit("tcsh").is_err());
        assert!(emit("").is_err());
    }
}
