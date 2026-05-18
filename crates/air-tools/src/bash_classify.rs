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
    let Some(log) = object
        .get("log")
        .and_then(Value::as_str)
        .map(str::to_string)
    else {
        return;
    };
    if cargo_test_log_has_only_zero_tests(&log) {
        object.insert("success".to_string(), Value::Bool(false));
        object.insert("verification_no_tests".to_string(), Value::Bool(true));
        object.insert(
            "verification_failure_reason".to_string(),
            Value::String("verification command matched zero tests".to_string()),
        );
        update_artifact_metadata(object, |metadata| {
            metadata.insert("success".to_string(), Value::Bool(false));
            metadata.insert("verification_no_tests".to_string(), Value::Bool(true));
            metadata.insert(
                "verification_failure_reason".to_string(),
                Value::String("verification command matched zero tests".to_string()),
            );
        });
    }
    if let Some(status) = echoed_exit_status(&log) {
        object.insert(
            "reported_exit_status".to_string(),
            Value::Number(status.into()),
        );
        if status != 0 {
            object.insert("success".to_string(), Value::Bool(false));
            object.insert("status".to_string(), Value::Number(status.into()));
            update_artifact_metadata(object, |metadata| {
                metadata.insert("success".to_string(), Value::Bool(false));
                metadata.insert("status".to_string(), Value::Number(status.into()));
                metadata.insert(
                    "reported_exit_status".to_string(),
                    Value::Number(status.into()),
                );
            });
        }
    }
    if echoed_missing_file_assertion(&log) {
        object.insert("success".to_string(), Value::Bool(false));
        object.insert(
            "verification_failure_reason".to_string(),
            Value::String("verification command reported missing file".to_string()),
        );
        update_artifact_metadata(object, |metadata| {
            metadata.insert("success".to_string(), Value::Bool(false));
            metadata.insert(
                "verification_failure_reason".to_string(),
                Value::String("verification command reported missing file".to_string()),
            );
        });
    }
}

fn update_artifact_metadata(
    object: &mut Map<String, Value>,
    mut update: impl FnMut(&mut Map<String, Value>),
) {
    if let Some(Value::Array(artifacts)) = object.get_mut("artifacts") {
        for artifact in artifacts {
            if let Some(metadata) = artifact.get_mut("metadata").and_then(Value::as_object_mut) {
                update(metadata);
            }
        }
    }
}

fn cargo_test_log_has_only_zero_tests(log: &str) -> bool {
    let counts = log
        .lines()
        .filter_map(cargo_running_test_count)
        .collect::<Vec<_>>();
    !counts.is_empty() && counts.iter().all(|count| *count == 0)
}

fn cargo_running_test_count(line: &str) -> Option<u64> {
    let line = line.trim();
    let rest = line.strip_prefix("running ")?;
    let mut parts = rest.split_whitespace();
    let count = parts.next()?.parse::<u64>().ok()?;
    let unit = parts.next()?;
    matches!(unit, "test" | "tests").then_some(count)
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

fn echoed_missing_file_assertion(log: &str) -> bool {
    log.lines()
        .map(str::trim)
        .any(|line| matches!(line, "NOT FOUND" | "MISSING"))
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
    if is_filesystem_assertion_command(command) {
        return true;
    }
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

fn is_filesystem_assertion_command(command: &str) -> bool {
    let trimmed = command.trim_start();
    let normalized = trimmed
        .trim_start_matches("bash -lc ")
        .trim_start_matches("bash -c ")
        .trim_start_matches('"')
        .trim_start_matches('\'')
        .trim_start();
    normalized.starts_with("test -f ")
        || normalized.starts_with("test -d ")
        || normalized.starts_with("test -s ")
        || normalized.starts_with("[ -f ")
        || normalized.starts_with("[ -d ")
        || normalized.starts_with("[ -s ")
        || normalized.starts_with("[[ -f ")
        || normalized.starts_with("[[ -d ")
        || normalized.starts_with("[[ -s ")
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
    use super::{is_verification_bash_command, normalize_bash_verification_result};
    use serde_json::{json, Map, Value};

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
            "node skills/code-agent/edit-fixture/test.js",
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

    #[test]
    fn file_assertion_commands_are_verification() {
        assert!(is_verification_bash_command(
            "test -f WEBAPP_TESTING_HELP.md",
            Some("Verify file exists")
        ));
        assert!(is_verification_bash_command(
            "test -f WEBAPP_TESTING_HELP.md && echo EXISTS || echo NOT FOUND",
            Some("Verify file exists")
        ));
        assert!(is_verification_bash_command(
            "[ -s output.md ]",
            Some("Verify file is non-empty")
        ));
    }

    #[test]
    fn echoed_missing_file_assertion_marks_verification_failed() {
        let mut output = json!({
            "success": true,
            "status": 0,
            "log": "NOT FOUND\n",
            "artifacts": [{
                "metadata": {
                    "success": true
                }
            }]
        })
        .as_object()
        .cloned()
        .unwrap_or_else(Map::new);

        normalize_bash_verification_result(&mut output);

        assert_eq!(output.get("success"), Some(&Value::Bool(false)));
        assert_eq!(
            output
                .get("artifacts")
                .and_then(Value::as_array)
                .and_then(|artifacts| artifacts.first())
                .and_then(|artifact| artifact.pointer("/metadata/success")),
            Some(&Value::Bool(false))
        );
    }

    #[test]
    fn cargo_test_zero_tests_is_not_successful_verification() {
        let mut output = json!({
            "success": true,
            "status": 0,
            "log": "\
running 0 tests\n\
\n\
test result: ok. 0 passed; 0 failed; 0 ignored; 60 filtered out; finished in 0.00s\n\
\n\
running 0 tests\n\
\n\
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s\n",
            "artifacts": [{
                "metadata": {
                    "success": true
                }
            }]
        })
        .as_object()
        .cloned()
        .unwrap_or_else(Map::new);

        normalize_bash_verification_result(&mut output);

        assert_eq!(output.get("success"), Some(&Value::Bool(false)));
        assert_eq!(
            output.get("verification_no_tests"),
            Some(&Value::Bool(true))
        );
        assert_eq!(
            output
                .get("artifacts")
                .and_then(Value::as_array)
                .and_then(|artifacts| artifacts.first())
                .and_then(|artifact| artifact.pointer("/metadata/success")),
            Some(&Value::Bool(false))
        );
    }

    #[test]
    fn cargo_test_zero_doctests_after_unit_test_remains_successful() {
        let mut output = json!({
            "success": true,
            "status": 0,
            "log": "\
running 1 test\n\
test tests::example ... ok\n\
\n\
test result: ok. 1 passed; 0 failed; 0 ignored; 59 filtered out; finished in 0.00s\n\
\n\
running 0 tests\n\
\n\
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s\n"
        })
        .as_object()
        .cloned()
        .unwrap_or_else(Map::new);

        normalize_bash_verification_result(&mut output);

        assert_eq!(output.get("success"), Some(&Value::Bool(true)));
        assert_eq!(output.get("verification_no_tests"), None);
    }
}
