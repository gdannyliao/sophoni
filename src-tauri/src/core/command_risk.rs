use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandRisk {
    Low,
    High,
}

pub fn classify_command(command: &str, _workspace_root: &str) -> CommandRisk {
    let normalized = command.trim().to_lowercase();

    if normalized.is_empty() {
        return CommandRisk::High;
    }

    if has_shell_structure_risk(&normalized) {
        return CommandRisk::High;
    }

    if has_rg_execution_risk(&normalized) {
        return CommandRisk::High;
    }

    if normalized.starts_with("rg ") || normalized == "rg" {
        return CommandRisk::Low;
    }

    let high_risk_markers = [
        "rm ",
        "rm -",
        "mv ",
        "curl ",
        "wget ",
        "| sh",
        "| bash",
        "npm install",
        "pnpm install",
        "yarn install",
        "cargo install",
        "brew install",
        "git reset",
        "git clean",
        "sudo ",
        "chmod ",
        "chown ",
        "> /",
    ];

    if high_risk_markers
        .iter()
        .any(|marker| normalized.contains(marker))
    {
        return CommandRisk::High;
    }

    let exact_low_risk = [
        "ls",
        "ls -la",
        "git status",
        "git diff",
        "npm test",
        "pnpm test",
        "yarn test",
        "cargo test",
    ];

    if exact_low_risk.contains(&normalized.as_str()) {
        return CommandRisk::Low;
    }

    CommandRisk::High
}

fn has_shell_structure_risk(normalized: &str) -> bool {
    let shell_structure_markers = [";", "&&", "||", "|", ">", "<", "&", "\n", "`", "$("];

    shell_structure_markers
        .iter()
        .any(|marker| normalized.contains(marker))
}

fn has_rg_execution_risk(normalized: &str) -> bool {
    if !(normalized.starts_with("rg ") || normalized == "rg") {
        return false;
    }

    shell_words(normalized)
        .iter()
        .any(|arg| arg == "--pre" || arg.starts_with("--pre="))
}

fn shell_words(command: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;

    for character in command.chars() {
        match quote {
            Some(active_quote) if character == active_quote => {
                quote = None;
            }
            Some(_) => current.push(character),
            None if character == '\'' || character == '"' => {
                quote = Some(character);
            }
            None if character.is_whitespace() => {
                if !current.is_empty() {
                    words.push(std::mem::take(&mut current));
                }
            }
            None => current.push(character),
        }
    }

    if !current.is_empty() {
        words.push(current);
    }

    words
}
