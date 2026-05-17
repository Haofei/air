use serde_json::{Map, Value};

pub(super) fn forbidden_git_workspace_command(command: &str) -> Option<String> {
    let command = command.to_ascii_lowercase();
    let tokens = command
        .split(is_shell_command_boundary)
        .filter(|token| !token.is_empty())
        .collect::<Vec<_>>();
    for (index, token) in tokens.iter().enumerate() {
        if *token != "git" {
            continue;
        }
        let Some(subcommand) = git_subcommand_after_options(&tokens[index + 1..]) else {
            continue;
        };
        if is_forbidden_git_subcommand(subcommand) {
            return Some(subcommand.to_string());
        }
    }
    None
}

pub(super) fn git_subcommand_after_options<'a>(tokens: &'a [&'a str]) -> Option<&'a str> {
    let mut index = 0;
    while let Some(token) = tokens.get(index).copied() {
        if !token.starts_with('-') {
            return Some(token);
        }
        index += 1;
        if matches!(token, "-c" | "-C" | "--git-dir" | "--work-tree") {
            index += 1;
        }
    }
    None
}

fn is_forbidden_git_subcommand(subcommand: &str) -> bool {
    matches!(
        subcommand,
        "add"
            | "am"
            | "apply"
            | "checkout"
            | "cherry-pick"
            | "clean"
            | "commit"
            | "init"
            | "merge"
            | "mv"
            | "pull"
            | "push"
            | "rebase"
            | "reset"
            | "restore"
            | "revert"
            | "rm"
            | "stash"
            | "switch"
            | "tag"
            | "worktree"
    )
}

fn is_shell_command_boundary(character: char) -> bool {
    character.is_whitespace() || matches!(character, ';' | '&' | '|' | '(' | ')' | '{' | '}')
}

pub(super) fn normalize_bash_verification_result(object: &mut Map<String, Value>) {
    let Some(log) = object.get("log").and_then(Value::as_str) else {
        return;
    };
    let Some(status) = echoed_exit_status(log) else {
        return;
    };
    object.insert(
        "reported_exit_status".to_string(),
        Value::Number(status.into()),
    );
    if status != 0 {
        object.insert("success".to_string(), Value::Bool(false));
        object.insert("status".to_string(), Value::Number(status.into()));
        if let Some(Value::Array(artifacts)) = object.get_mut("artifacts") {
            for artifact in artifacts {
                if let Some(metadata) = artifact.get_mut("metadata").and_then(Value::as_object_mut)
                {
                    metadata.insert("success".to_string(), Value::Bool(false));
                    metadata.insert("status".to_string(), Value::Number(status.into()));
                    metadata.insert(
                        "reported_exit_status".to_string(),
                        Value::Number(status.into()),
                    );
                }
            }
        }
    }
}

fn echoed_exit_status(log: &str) -> Option<i64> {
    log.lines().rev().find_map(|line| {
        let trimmed = line.trim();
        let value = ["EXIT:", "exit:", "exit code:"]
            .iter()
            .find_map(|marker| {
                trimmed
                    .find(marker)
                    .map(|index| &trimmed[index + marker.len()..])
            })?
            .trim();
        let value = leading_i64_text(value)?;
        value.parse::<i64>().ok()
    })
}

fn leading_i64_text(value: &str) -> Option<&str> {
    let value = value
        .trim_start_matches(|character: char| !(character.is_ascii_digit() || character == '-'));
    let mut end = 0usize;
    for (index, character) in value.char_indices() {
        if index == 0 && character == '-' {
            end = character.len_utf8();
            continue;
        }
        if !character.is_ascii_digit() {
            break;
        }
        end = index + character.len_utf8();
    }
    if end == 0 || value[..end].chars().all(|character| character == '-') {
        None
    } else {
        Some(&value[..end])
    }
}

pub(super) fn is_verification_bash_command(command: &str, description: Option<&str>) -> bool {
    let command = command.to_ascii_lowercase();
    let description = description.unwrap_or_default().to_ascii_lowercase();

    if is_inspection_bash_command(&command) {
        return false;
    }
    if command.contains("--no-run") {
        return false;
    }

    if is_command_verification(&command) {
        return true;
    }

    is_ambiguous_verification_command(&command)
        && [
            "verification",
            "verify",
            "retest",
            "test",
            "tests",
            "check",
            "compile",
            "typecheck",
            "type check",
            "lint",
        ]
        .iter()
        .any(|needle| description.contains(needle))
}

fn is_command_verification(command: &str) -> bool {
    if command.contains("cargo fmt") {
        return command.contains("--check");
    }
    if command.contains("prettier") && command.contains("--write") {
        return false;
    }
    if command.contains("eslint") && command.contains("--fix") {
        return false;
    }
    if command.trim_start().starts_with("node ") && command.contains("test") {
        return true;
    }

    [
        "cargo test",
        "cargo check",
        "cargo clippy",
        "go test",
        "make check",
        "make test",
        "npm test",
        "npm run check",
        "npm run lint",
        "npm run test",
        "npm run typecheck",
        "pnpm check",
        "pnpm lint",
        "pnpm test",
        "pnpm typecheck",
        "python -m py_compile",
        "python3 -m py_compile",
        "pytest",
        "yarn check",
        "yarn lint",
        "yarn test",
        "yarn typecheck",
    ]
    .iter()
    .any(|needle| command.contains(needle))
}

fn is_ambiguous_verification_command(command: &str) -> bool {
    ["verification", "verify"]
        .iter()
        .any(|needle| command.contains(needle))
}

fn is_inspection_bash_command(command: &str) -> bool {
    let command = command.trim_start();
    [
        "awk ",
        "cat ",
        "echo ",
        "find ",
        "git diff",
        "git ls-files",
        "git show",
        "git status",
        "grep ",
        "head ",
        "ls ",
        "printf ",
        "rg ",
        "sed ",
        "tail ",
        "wc ",
    ]
    .iter()
    .any(|prefix| command.starts_with(prefix))
}

#[cfg(test)]
mod tests {
    use super::is_verification_bash_command;

    #[test]
    fn verification_classification_is_command_first() {
        assert!(is_verification_bash_command(
            "cargo test -q",
            Some("Run command")
        ));
        assert!(is_verification_bash_command(
            "python3 -m py_compile check.py",
            Some("Run command")
        ));
        assert!(is_verification_bash_command(
            "node examples/code-agent/edit-fixture/test.js",
            Some("Run command")
        ));
        assert!(!is_verification_bash_command(
            "true",
            Some("Run verification")
        ));
    }

    #[test]
    fn mutating_format_commands_are_not_verification() {
        assert!(!is_verification_bash_command(
            "cargo fmt",
            Some("format code")
        ));
        assert!(is_verification_bash_command(
            "cargo fmt --check",
            Some("format check")
        ));
        assert!(!is_verification_bash_command(
            "prettier --write src/app.ts",
            Some("verify formatting")
        ));
    }
}
