//! `--allow-run` is trust in the granted program: these can run arbitrary code with the
//! caller's authority, so granting one is the same as granting everything (audit of VELYR-83).

use super::Scope;

/// Programs (and shells) that execute what they are told, or tell others to.
const FULL_TRUST: [&str; 22] = [
    "git",
    "find",
    "tar",
    "ssh",
    "env",
    "xargs",
    "node",
    "velyr",
    "cat",
    "perl",
    "ruby",
    "sh",
    "bash",
    "zsh",
    "dash",
    "ksh",
    "fish",
    "csh",
    "tcsh",
    "cmd",
    "powershell",
    "pwsh",
];

fn grants_full_trust(program: &str) -> bool {
    let name = program.rsplit(['/', '\\']).next().unwrap_or(program);
    let name = name.to_ascii_lowercase();
    let name = name.trim_end_matches(".exe");
    program == "*" || FULL_TRUST.contains(&name) || name.starts_with("python")
}

/// The warning for an effective `allow-run` scope, if it grants full trust.
pub fn warning(scope: &Scope) -> Option<String> {
    let list = scope.as_ref()?;
    if list.is_empty() {
        return Some(
            "warning: --allow-run without a list lets the script start any program, which is \
             full trust: those programs run with your authority"
                .to_owned(),
        );
    }
    let risky: Vec<&str> = list
        .iter()
        .map(String::as_str)
        .filter(|p| grants_full_trust(p))
        .collect();
    (!risky.is_empty()).then(|| {
        format!(
            "warning: --allow-run={} is full trust: {} can run arbitrary code with your \
             authority; grant only programs you would run yourself",
            risky.join(","),
            if risky.len() == 1 { "it" } else { "they" }
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope(items: &[&str]) -> Scope {
        Some(items.iter().map(|s| (*s).to_owned()).collect())
    }

    #[test]
    fn full_trust_programs_warn_by_name_or_path() {
        for program in [
            "sh",
            "/bin/bash",
            "git",
            "python3",
            "Python.exe",
            "PowerShell.exe",
        ] {
            let text = warning(&scope(&[program])).unwrap_or_default();
            assert!(text.contains(program), "{program}: {text}");
        }
    }

    #[test]
    fn harmless_programs_and_no_flag_do_not_warn() {
        assert!(warning(&scope(&["echo", "ls"])).is_none());
        assert!(warning(&None).is_none());
    }

    #[test]
    fn a_bare_flag_and_a_star_warn() {
        assert!(warning(&scope(&[])).is_some());
        assert!(warning(&scope(&["*"])).is_some());
    }

    #[test]
    fn only_the_risky_entries_are_named() {
        let text = warning(&scope(&["echo", "ssh"])).unwrap_or_default();
        assert!(text.contains("--allow-run=ssh "), "{text}");
    }
}
