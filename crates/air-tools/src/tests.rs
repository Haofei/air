use super::*;

fn temp_dir(prefix: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "{prefix}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&path).unwrap();
    path
}

fn write_config(dir: &Path, content: &str) -> PathBuf {
    let path = dir.join("tools.json");
    fs::write(&path, content).unwrap();
    path
}

#[test]
fn todo_write_returns_a_structured_artifact() {
    let dir = temp_dir("air-tools-todo-write");
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "todo.write": {
                  "kind": "todo_write",
                  "capability": "task.progress",
                  "max_items": 4,
                  "max_content_chars": 80
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let output = tools
        .call_tool(
            "todo.write",
            &json!({
                "todos": [
                    {
                        "id": "inspect",
                        "content": "Inspect repository context",
                        "status": "completed",
                        "priority": "high"
                    },
                    {
                        "id": "fix",
                        "content": "Apply the bounded fix",
                        "status": "in_progress",
                        "priority": "high"
                    },
                    {
                        "id": "verify",
                        "content": "Run the allowlisted verification",
                        "status": "pending",
                        "priority": "medium"
                    }
                ]
            }),
        )
        .unwrap();

    assert_eq!(output["total"], json!(3));
    assert_eq!(output["open_count"], json!(2));
    assert_eq!(output["in_progress_count"], json!(1));
    assert_eq!(output["artifacts"][0]["kind"], json!("todo_list"));
    assert_eq!(tools.tool_capability("todo.write"), Some("task.progress"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn todo_read_returns_current_todo_list_after_write() {
    let dir = temp_dir("air-tools-todo-read");
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "todo.write": {
                  "kind": "todo_write",
                  "capability": "task.progress"
                },
                "todo.read": {
                  "kind": "todo_read",
                  "capability": "task.progress"
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let empty = tools.call_tool("todo.read", &json!({})).unwrap();
    assert_eq!(empty["total"], json!(0));
    assert_eq!(empty["open_count"], json!(0));
    assert_eq!(
        empty["artifacts"][0]["metadata"]["provider"],
        json!("todo_read")
    );

    tools
        .call_tool(
            "todo.write",
            &json!({
                "todos": [
                    {
                        "id": "inspect",
                        "content": "Inspect repository context",
                        "status": "completed",
                        "priority": "high"
                    },
                    {
                        "id": "verify",
                        "content": "Run verification",
                        "status": "pending",
                        "priority": "medium"
                    }
                ]
            }),
        )
        .unwrap();

    let output = tools.call_tool("todo.read", &json!({})).unwrap();
    assert_eq!(output["total"], json!(2));
    assert_eq!(output["open_count"], json!(1));
    assert_eq!(output["completed_count"], json!(1));
    assert_eq!(output["todos"][1]["id"], json!("verify"));
    assert_eq!(tools.tool_capability("todo.read"), Some("task.progress"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn context_measure_triggers_when_payload_crosses_threshold() {
    let dir = temp_dir("air-tools-context-measure");
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "context.measure": {
                  "kind": "context_measure",
                  "capability": "context.manage",
                  "max_context_chars": 100,
                  "threshold_percent": 80
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let output = tools
        .call_tool(
            "context.measure",
            &json!({"payload": {"large": "x".repeat(100), "small": "ok"}}),
        )
        .unwrap();

    assert_eq!(output["should_compact"], json!(true));
    assert_eq!(output["threshold_chars"], json!(80));
    assert_eq!(output["artifacts"][0]["kind"], json!("context_measure"));
    assert_eq!(
        tools.tool_capability("context.measure"),
        Some("context.manage")
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn context_measure_defaults_to_200k_context_window() {
    let dir = temp_dir("air-tools-context-measure-default");
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "context.measure": {
                  "kind": "context_measure",
                  "capability": "context.manage"
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let output = tools
        .call_tool("context.measure", &json!({"payload": {"small": "ok"}}))
        .unwrap();

    assert_eq!(output["max_context_chars"], json!(200000));
    assert_eq!(output["threshold_percent"], json!(80));
    assert_eq!(output["threshold_chars"], json!(160000));
    assert_eq!(output["should_compact"], json!(false));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn context_measure_respects_input_threshold_override() {
    let dir = temp_dir("air-tools-context-measure-override");
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "context.measure": {
                  "kind": "context_measure",
                  "max_context_chars": 1000,
                  "threshold_percent": 80
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let output = tools
        .call_tool(
            "context.measure",
            &json!({
                "payload": {"small": "ok"},
                "max_context_chars": 100,
                "threshold_percent": 1
            }),
        )
        .unwrap();

    assert_eq!(output["should_compact"], json!(true));
    assert_eq!(output["threshold_percent"], json!(1));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn artifact_validate_rejects_unregistered_citations_when_configured() {
    let dir = temp_dir("air-tools-artifact-validate");
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "artifact.validate": {
                  "kind": "artifact_validate",
                  "capability": "provenance.validate",
                  "fail_on_missing": true
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let error = tools
        .call_tool(
            "artifact.validate",
            &json!({
                "evidence": {
                    "search": {
                        "artifacts": [
                            {
                                "id": "docs-command-run-safety",
                                "kind": "doc_chunk",
                                "title": "Command runner safety"
                            }
                        ]
                    }
                },
                "citations": {
                    "findings": [
                        {
                            "source_ids": [
                                "docs-command-run-safety",
                                "missing-source"
                            ]
                        }
                    ]
                }
            }),
        )
        .unwrap_err();

    assert!(format!("{error}").contains("missing-source"));
    assert_eq!(
        tools.tool_capability("artifact.validate"),
        Some("provenance.validate")
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn artifact_validate_accepts_registered_artifact_ids() {
    let dir = temp_dir("air-tools-artifact-validate-ok");
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "artifact.validate": {
                  "kind": "artifact_validate",
                  "capability": "provenance.validate",
                  "fail_on_missing": true
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let output = tools
        .call_tool(
            "artifact.validate",
            &json!({
                "registered_ids": ["manual-source"],
                "evidence": {
                    "artifacts": [
                        {
                            "id": "docs-command-run-safety",
                            "kind": "doc_chunk",
                            "title": "Command runner safety"
                        }
                    ]
                },
                "citations": {
                    "source_ids": [
                        "docs-command-run-safety",
                        "manual-source"
                    ],
                    "summary": "This string is not treated as a citation."
                }
            }),
        )
        .unwrap();

    assert_eq!(output["valid"], json!(true));
    assert_eq!(output["missing_ids"], json!([]));
    assert_eq!(output["cited_ids"].as_array().unwrap().len(), 2);
    assert_eq!(output["artifacts"][0]["kind"], json!("artifact_validation"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn todo_write_rejects_multiple_in_progress_items() {
    let dir = temp_dir("air-tools-todo-write-invalid");
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "todo.write": {
                  "kind": "todo_write",
                  "capability": "task.progress"
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let error = tools
        .call_tool(
            "todo.write",
            &json!({
                "todos": [
                    {
                        "id": "one",
                        "content": "First task",
                        "status": "in_progress",
                        "priority": "high"
                    },
                    {
                        "id": "two",
                        "content": "Second task",
                        "status": "in_progress",
                        "priority": "medium"
                    }
                ]
            }),
        )
        .unwrap_err();

    assert!(error.to_string().contains("at most one in_progress"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn file_read_reads_inside_configured_base_dir() {
    let dir = temp_dir("air-tools-file-read");
    let docs = dir.join("docs");
    fs::create_dir_all(&docs).unwrap();
    fs::write(docs.join("note.txt"), "hello from AIR tools").unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "file.read": {
                  "kind": "file_read",
                  "capability": "file.read",
                  "base_dir": "docs",
                  "max_bytes": 8
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let output = tools
        .call_tool("file.read", &json!({"path": "note.txt"}))
        .unwrap();

    assert_eq!(output["content"], json!("hello fr"));
    assert_eq!(output["truncated"], json!(true));
    assert_eq!(output["artifacts"][0]["kind"], json!("file_span"));
    assert!(output["artifacts"][0]["id"]
        .as_str()
        .unwrap()
        .starts_with("file:"));
    assert_eq!(tools.tool_capability("file.read"), Some("file.read"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn file_read_many_reads_multiple_files_with_artifacts() {
    let dir = temp_dir("air-tools-file-read-many");
    fs::write(dir.join("one.txt"), "alpha\nbeta\n").unwrap();
    fs::write(dir.join("two.txt"), "first\nneedle\nlast\n").unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "file.read_many": {
                  "kind": "file_read_many",
                  "capability": "file.read",
                  "base_dir": ".",
                  "max_files": 3,
                  "max_bytes": 1024
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let output = tools
        .call_tool(
            "file.read_many",
            &json!({
                "files": [
                    "one.txt",
                    {
                        "path": "two.txt",
                        "contains": "needle",
                        "context_lines": 1,
                        "line_numbers": true
                    }
                ]
            }),
        )
        .unwrap();

    assert_eq!(output["file_count"], json!(2));
    assert_eq!(output["files"][0]["content"], json!("alpha\nbeta\n"));
    assert_eq!(output["max_bytes_per_file"], json!(1024));
    assert_eq!(output["files"][1]["match_line"], json!(2));
    assert_eq!(
        output["files"][1]["numbered_content"],
        json!("00001| first\n00002| needle\n00003| last")
    );
    assert_eq!(output["artifacts"].as_array().unwrap().len(), 2);
    assert_eq!(tools.tool_capability("file.read_many"), Some("file.read"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn file_read_many_supports_bounded_per_call_max_bytes() {
    let dir = temp_dir("air-tools-file-read-many-max-bytes");
    fs::write(dir.join("one.txt"), "abcdefghijklmnopqrstuvwxyz\n").unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "file.read_many": {
                  "kind": "file_read_many",
                  "capability": "file.read",
                  "base_dir": ".",
                  "max_files": 3,
                  "max_bytes": 12
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let output = tools
        .call_tool(
            "file.read_many",
            &json!({"files": ["one.txt"], "max_bytes_per_file": 5}),
        )
        .unwrap();
    assert_eq!(output["max_bytes_per_file"], json!(5));
    assert_eq!(output["files"][0]["content"], json!("abcde"));
    assert_eq!(output["files"][0]["truncated"], json!(true));

    let capped = tools
        .call_tool(
            "file.read_many",
            &json!({"files": ["one.txt"], "max_bytes_per_file": 99}),
        )
        .unwrap();
    assert_eq!(capped["max_bytes_per_file"], json!(12));
    assert_eq!(capped["files"][0]["content"], json!("abcdefghijkl"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn file_read_many_rejects_more_than_configured_max_files() {
    let dir = temp_dir("air-tools-file-read-many-max");
    fs::write(dir.join("one.txt"), "one").unwrap();
    fs::write(dir.join("two.txt"), "two").unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "file.read_many": {
                  "kind": "file_read_many",
                  "capability": "file.read",
                  "base_dir": ".",
                  "max_files": 1
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let error = tools
        .call_tool("file.read_many", &json!({"files": ["one.txt", "two.txt"]}))
        .unwrap_err();

    assert!(error.to_string().contains("at most 1 files"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn file_read_many_allows_empty_file_list() {
    let dir = temp_dir("air-tools-file-read-many-empty");
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "file.read_many": {
                  "kind": "file_read_many",
                  "capability": "file.read",
                  "base_dir": "."
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let output = tools
        .call_tool("file.read_many", &json!({"files": []}))
        .unwrap();

    assert_eq!(output["file_count"], json!(0));
    assert_eq!(output["files"], json!([]));
    assert_eq!(output["artifacts"], json!([]));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn file_read_supports_bounded_per_call_max_bytes() {
    let dir = temp_dir("air-tools-file-read-per-call-max-bytes");
    fs::write(dir.join("note.txt"), "abcdefghijklmnopqrstuvwxyz\n").unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "file.read": {
                  "kind": "file_read",
                  "capability": "file.read",
                  "base_dir": ".",
                  "max_bytes": 16
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let output = tools
        .call_tool("file.read", &json!({"path": "note.txt", "max_bytes": 8}))
        .unwrap();
    assert_eq!(output["content"], json!("abcdefgh"));
    assert_eq!(output["max_bytes"], json!(8));
    assert_eq!(output["truncated"], json!(true));

    let capped = tools
        .call_tool("file.read", &json!({"path": "note.txt", "max_bytes": 99}))
        .unwrap();
    assert_eq!(capped["content"], json!("abcdefghijklmnop"));
    assert_eq!(capped["max_bytes"], json!(16));
    assert_eq!(capped["truncated"], json!(true));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn file_search_returns_regex_matches_with_context() {
    let dir = temp_dir("air-tools-file-search");
    fs::write(
        dir.join("note.txt"),
        "alpha\nfirst needle\nmiddle\nsecond needle\nomega\n",
    )
    .unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "file.search": {
                  "kind": "file_search",
                  "capability": "file.read",
                  "base_dir": ".",
                  "max_bytes": 4096,
                  "max_matches": 8,
                  "max_context_lines": 4
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let output = tools
        .call_tool(
            "file.search",
            &json!({"path": "note.txt", "pattern": "needle$", "context_lines": 1}),
        )
        .unwrap();

    assert_eq!(output["pattern"], json!("needle$"));
    assert_eq!(output["match_count"], json!(2));
    assert_eq!(output["total_lines"], json!(5));
    assert_eq!(output["truncated"], json!(false));
    assert_eq!(output["matches"][0]["line_number"], json!(2));
    assert_eq!(output["matches"][0]["line"], json!("first needle"));
    assert_eq!(output["matches"][0]["before"][0]["line_number"], json!(1));
    assert_eq!(output["matches"][0]["before"][0]["line"], json!("alpha"));
    assert_eq!(output["matches"][0]["after"][0]["line_number"], json!(3));
    assert_eq!(output["matches"][0]["after"][0]["line"], json!("middle"));
    assert_eq!(output["artifacts"][0]["kind"], json!("file_search"));
    assert_eq!(tools.tool_capability("file.search"), Some("file.read"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn file_search_accepts_query_alias_for_pattern() {
    let dir = temp_dir("air-tools-file-search-query-alias");
    fs::write(dir.join("note.txt"), "alpha\nneedle\n").unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "file.search": {
                  "kind": "file_search",
                  "capability": "file.read",
                  "base_dir": "."
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let output = tools
        .call_tool(
            "file.search",
            &json!({"path": "note.txt", "query": "needle"}),
        )
        .unwrap();

    assert_eq!(output["pattern"], json!("needle"));
    assert_eq!(output["pattern_source"], json!("query"));
    assert_eq!(output["match_count"], json!(1));
    assert_eq!(
        output["artifacts"][0]["metadata"]["pattern_source"],
        json!("query")
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn file_search_recurses_within_directory_paths() {
    let dir = temp_dir("air-tools-file-search-directory");
    fs::create_dir_all(dir.join("src/nested")).unwrap();
    fs::write(
        dir.join("src/math.js"),
        "function add(a, b) {\n  return a - b;\n}\n",
    )
    .unwrap();
    fs::write(
        dir.join("src/nested/other.js"),
        "function subtract(a, b) {}\n",
    )
    .unwrap();
    fs::write(dir.join("README.md"), "add documentation\n").unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "file.search": {
                  "kind": "file_search",
                  "capability": "file.read",
                  "base_dir": ".",
                  "max_bytes": 4096,
                  "max_matches": 8,
                  "max_context_lines": 2
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let output = tools
        .call_tool(
            "file.search",
            &json!({"path": "src", "pattern": "add", "context_lines": 1}),
        )
        .unwrap();

    assert_eq!(output["directory"], json!(true));
    assert_eq!(output["match_count"], json!(1));
    assert_eq!(output["matches"][0]["path"], json!("src/math.js"));
    assert_eq!(output["matches"][0]["line_number"], json!(1));
    assert_eq!(output["searched_file_count"], json!(2));
    assert_eq!(
        output["artifacts"][0]["metadata"]["searched_file_count"],
        json!(2)
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn file_search_rejects_invalid_regex() {
    let dir = temp_dir("air-tools-file-search-invalid-regex");
    fs::write(dir.join("note.txt"), "alpha\n").unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "file.search": {
                  "kind": "file_search",
                  "capability": "file.read",
                  "base_dir": "."
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let error = tools
        .call_tool("file.search", &json!({"path": "note.txt", "pattern": "["}))
        .unwrap_err();

    assert!(error.to_string().contains("invalid regex"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn file_search_truncates_long_structured_lines() {
    let dir = temp_dir("air-tools-file-search-long-lines");
    fs::write(
        dir.join("note.txt"),
        "before needle abcdefghijklmnopqrstuvwxyz after\n",
    )
    .unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "file.search": {
                  "kind": "file_search",
                  "capability": "file.read",
                  "base_dir": ".",
                  "max_bytes": 4096,
                  "max_matches": 8,
                  "max_context_lines": 4,
                  "max_line_chars": 12
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let output = tools
        .call_tool(
            "file.search",
            &json!({"path": "note.txt", "pattern": "needle"}),
        )
        .unwrap();

    assert_eq!(output["match_count"], json!(1));
    assert_eq!(output["max_line_chars"], json!(12));
    assert_eq!(output["line_truncated"], json!(true));
    assert_eq!(output["truncated"], json!(true));
    assert_eq!(output["matches"][0]["line"], json!("before needl"));
    assert_eq!(output["matches"][0]["line_truncated"], json!(true));
    assert_eq!(
        output["artifacts"][0]["metadata"]["line_truncated"],
        json!(true)
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn file_search_supports_bounded_per_call_limits() {
    let dir = temp_dir("air-tools-file-search-per-call-limits");
    fs::write(
        dir.join("note.txt"),
        "needle-abcdef\nneedle-bcdefg\nneedle-cdefgh\nneedle-defghi\n",
    )
    .unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "file.search": {
                  "kind": "file_search",
                  "capability": "file.read",
                  "base_dir": ".",
                  "max_bytes": 4096,
                  "max_matches": 3,
                  "max_context_lines": 4,
                  "max_line_chars": 10
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let output = tools
        .call_tool(
            "file.search",
            &json!({
                "path": "note.txt",
                "pattern": "needle",
                "max_matches": 2,
                "max_line_chars": 8
            }),
        )
        .unwrap();
    assert_eq!(output["match_count"], json!(4));
    assert_eq!(output["returned_match_count"], json!(2));
    assert_eq!(output["max_matches"], json!(2));
    assert_eq!(output["max_line_chars"], json!(8));
    assert_eq!(output["matches"][0]["line"], json!("needle-a"));
    assert_eq!(output["truncated"], json!(true));

    let capped = tools
        .call_tool(
            "file.search",
            &json!({
                "path": "note.txt",
                "pattern": "needle",
                "max_matches": 99,
                "max_line_chars": 99
            }),
        )
        .unwrap();
    assert_eq!(capped["returned_match_count"], json!(3));
    assert_eq!(capped["max_matches"], json!(3));
    assert_eq!(capped["max_line_chars"], json!(10));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn file_search_satisfies_read_before_file_ops_edit() {
    let dir = temp_dir("air-tools-file-search-read-before-edit");
    fs::write(dir.join("note.txt"), "hello AIR\n").unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "file.search": {
                  "kind": "file_search",
                  "capability": "file.read",
                  "base_dir": "."
                },
                "file.ops": {
                  "kind": "file_ops",
                  "capability": "file.write",
                  "base_dir": ".",
                  "require_read": true
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    tools
        .call_tool(
            "file.search",
            &json!({"path": "note.txt", "pattern": "AIR"}),
        )
        .unwrap();
    let output = tools
        .call_tool(
            "file.ops",
            &json!({
                "operations": [{
                    "kind": "edit",
                    "path": "note.txt",
                    "old_string": "AIR",
                    "new_string": "agent IR"
                }]
            }),
        )
        .unwrap();

    assert_eq!(output["applied"], json!(true));
    assert_eq!(
        fs::read_to_string(dir.join("note.txt")).unwrap(),
        "hello agent IR\n"
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn file_search_read_snapshot_rejects_stale_file_ops_edit() {
    let dir = temp_dir("air-tools-file-search-stale-edit");
    fs::write(dir.join("note.txt"), "hello AIR\n").unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "file.search": {
                  "kind": "file_search",
                  "capability": "file.read",
                  "base_dir": "."
                },
                "file.ops": {
                  "kind": "file_ops",
                  "capability": "file.write",
                  "base_dir": ".",
                  "require_read": true
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();
    tools
        .call_tool(
            "file.search",
            &json!({"path": "note.txt", "pattern": "AIR"}),
        )
        .unwrap();
    std::thread::sleep(Duration::from_millis(20));
    fs::write(dir.join("note.txt"), "outside AIR change\n").unwrap();

    let error = tools
        .call_tool(
            "file.ops",
            &json!({
                "operations": [{
                    "kind": "edit",
                    "path": "note.txt",
                    "old_string": "AIR",
                    "new_string": "agent IR"
                }]
            }),
        )
        .unwrap_err();

    assert!(error
        .to_string()
        .contains("modified after it was last read"));
    assert_eq!(
        fs::read_to_string(dir.join("note.txt")).unwrap(),
        "outside AIR change\n"
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn file_read_rejects_path_outside_base_dir() {
    let dir = temp_dir("air-tools-file-read-boundary");
    let docs = dir.join("docs");
    fs::create_dir_all(&docs).unwrap();
    fs::write(dir.join("secret.txt"), "secret").unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "file.read": {
                  "kind": "file_read",
                  "capability": "file.read",
                  "base_dir": "docs"
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let error = tools
        .call_tool("file.read", &json!({"path": "../secret.txt"}))
        .unwrap_err();

    assert!(error.to_string().contains("outside configured base_dir"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn file_read_rejects_binary_content() {
    let dir = temp_dir("air-tools-file-read-binary");
    fs::write(dir.join("blob.bin"), [0, 159, 146, 150, 0, 1]).unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "file.read": {
                  "kind": "file_read",
                  "capability": "file.read",
                  "base_dir": "."
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let error = tools
        .call_tool("file.read", &json!({"path": "blob.bin"}))
        .unwrap_err();

    assert!(error.to_string().contains("appears to be binary"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn file_read_rejects_non_utf8_text() {
    let dir = temp_dir("air-tools-file-read-non-utf8");
    fs::write(dir.join("latin1.txt"), [b'h', b'i', 0xff]).unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "file.read": {
                  "kind": "file_read",
                  "capability": "file.read",
                  "base_dir": "."
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let error = tools
        .call_tool("file.read", &json!({"path": "latin1.txt"}))
        .unwrap_err();

    assert!(error.to_string().contains("not valid UTF-8"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn file_read_can_return_numbered_content() {
    let dir = temp_dir("air-tools-file-read-numbered");
    fs::write(dir.join("note.txt"), "one\ntwo\nthree\n").unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "file.read": {
                  "kind": "file_read",
                  "capability": "file.read",
                  "base_dir": "."
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let output = tools
        .call_tool(
            "file.read",
            &json!({"path": "note.txt", "start_line": 2, "line_numbers": true}),
        )
        .unwrap();

    assert_eq!(output["content"], json!("two\nthree"));
    assert_eq!(output["line_numbers"], json!(true));
    assert_eq!(
        output["numbered_content"],
        json!("00002| two\n00003| three")
    );
    assert_eq!(
        output["artifacts"][0]["metadata"]["line_numbers"],
        json!(true)
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn file_read_range_defaults_to_numbered_content() {
    let dir = temp_dir("air-tools-file-read-range-numbered-default");
    fs::write(dir.join("note.txt"), "one\ntwo\nthree\n").unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "file.read": {
                  "kind": "file_read",
                  "capability": "file.read",
                  "base_dir": "."
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let output = tools
        .call_tool("file.read", &json!({"path": "note.txt", "start_line": 2}))
        .unwrap();

    assert_eq!(output["content"], json!("two\nthree"));
    assert_eq!(output["line_numbers"], json!(true));
    assert_eq!(output["line_numbers_defaulted"], json!(true));
    assert_eq!(
        output["numbered_content"],
        json!("00002| two\n00003| three")
    );
    assert_eq!(
        output["artifacts"][0]["metadata"]["line_numbers_defaulted"],
        json!(true)
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn file_read_accepts_lines_range_alias() {
    let dir = temp_dir("air-tools-file-read-lines-range");
    fs::write(dir.join("note.txt"), "one\ntwo\nthree\nfour\n").unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "file.read": {
                  "kind": "file_read",
                  "capability": "file.read",
                  "base_dir": "."
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let output = tools
        .call_tool("file.read", &json!({"path": "note.txt", "lines": "2-3"}))
        .unwrap();

    assert_eq!(output["content"], json!("two\nthree"));
    assert_eq!(output["start_line"], json!(2));
    assert_eq!(output["end_line"], json!(3));
    assert_eq!(output["line_numbers"], json!(true));
    assert_eq!(output["line_numbers_defaulted"], json!(true));
    assert_eq!(
        output["numbered_content"],
        json!("00002| two\n00003| three")
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn file_write_writes_inside_configured_base_dir() {
    let dir = temp_dir("air-tools-file-write");
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "file.write": {
                  "kind": "file_write",
                  "capability": "file.write",
                  "base_dir": ".",
                  "create_dirs": true,
                  "allow_overwrite": true,
                  "max_bytes": 1024
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let output = tools
        .call_tool(
            "file.write",
            &json!({"path": "site/index.html", "content": "<h1>AIR</h1>"}),
        )
        .unwrap();

    assert_eq!(
        fs::read_to_string(dir.join("site/index.html")).unwrap(),
        "<h1>AIR</h1>"
    );
    assert_eq!(output["created"], json!(true));
    assert_eq!(output["artifacts"][0]["kind"], json!("file_write"));
    assert_eq!(tools.tool_capability("file.write"), Some("file.write"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn file_write_rejects_parent_path_escape() {
    let dir = temp_dir("air-tools-file-write-boundary");
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "file.write": {
                  "kind": "file_write",
                  "capability": "file.write",
                  "base_dir": ".",
                  "create_dirs": true
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let error = tools
        .call_tool(
            "file.write",
            &json!({"path": "../escape.html", "content": "x"}),
        )
        .unwrap_err();

    assert!(error.to_string().contains("relative paths inside repo_dir"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn file_write_requires_read_before_overwrite_when_configured() {
    let dir = temp_dir("air-tools-file-write-read-first");
    fs::write(dir.join("note.txt"), "before").unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "file.read": {
                  "kind": "file_read",
                  "capability": "file.read",
                  "base_dir": "."
                },
                "file.write": {
                  "kind": "file_write",
                  "capability": "file.write",
                  "base_dir": ".",
                  "allow_overwrite": true,
                  "require_read": true
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let error = tools
        .call_tool(
            "file.write",
            &json!({"path": "note.txt", "content": "after"}),
        )
        .unwrap_err();
    assert!(error.to_string().contains("must be read before overwrite"));

    tools
        .call_tool("file.read", &json!({"path": "note.txt"}))
        .unwrap();
    let output = tools
        .call_tool(
            "file.write",
            &json!({"path": "note.txt", "content": "after"}),
        )
        .unwrap();

    assert_eq!(fs::read_to_string(dir.join("note.txt")).unwrap(), "after");
    assert_eq!(output["overwritten"], json!(true));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn file_write_rejects_stale_read_before_overwrite() {
    let dir = temp_dir("air-tools-file-write-stale-read");
    fs::write(dir.join("note.txt"), "before").unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "file.read": {
                  "kind": "file_read",
                  "capability": "file.read",
                  "base_dir": "."
                },
                "file.write": {
                  "kind": "file_write",
                  "capability": "file.write",
                  "base_dir": ".",
                  "allow_overwrite": true,
                  "require_read": true
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();
    tools
        .call_tool("file.read", &json!({"path": "note.txt"}))
        .unwrap();
    std::thread::sleep(Duration::from_millis(20));
    fs::write(dir.join("note.txt"), "outside change").unwrap();

    let error = tools
        .call_tool(
            "file.write",
            &json!({"path": "note.txt", "content": "agent change"}),
        )
        .unwrap_err();

    assert!(error
        .to_string()
        .contains("modified after it was last read"));
    assert_eq!(
        fs::read_to_string(dir.join("note.txt")).unwrap(),
        "outside change"
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn file_edit_replaces_unique_string_after_read() {
    let dir = temp_dir("air-tools-file-edit");
    fs::write(dir.join("note.txt"), "hello AIR\n").unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "file.read": {
                  "kind": "file_read",
                  "capability": "file.read",
                  "base_dir": "."
                },
                "file.edit": {
                  "kind": "file_edit",
                  "capability": "file.write",
                  "base_dir": ".",
                  "max_bytes": 1024
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let error = tools
        .call_tool(
            "file.edit",
            &json!({"path": "note.txt", "old_string": "AIR", "new_string": "agent IR"}),
        )
        .unwrap_err();
    assert!(error.to_string().contains("must be read before edit"));

    tools
        .call_tool("file.read", &json!({"path": "note.txt"}))
        .unwrap();
    let output = tools
        .call_tool(
            "file.edit",
            &json!({"path": "note.txt", "old_string": "AIR", "new_string": "agent IR"}),
        )
        .unwrap();

    assert_eq!(
        fs::read_to_string(dir.join("note.txt")).unwrap(),
        "hello agent IR\n"
    );
    assert_eq!(output["replacements"], json!(1));
    assert_eq!(output["diff_truncated"], json!(false));
    assert!(output["diff"].as_str().unwrap().contains("-AIR"));
    assert!(output["diff"].as_str().unwrap().contains("+agent IR"));
    assert_eq!(output["artifacts"][0]["content"], output["diff"]);
    assert_eq!(output["artifacts"][0]["kind"], json!("file_edit"));
    assert_eq!(tools.tool_capability("file.edit"), Some("file.write"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn file_edit_rejects_diff_that_exceeds_max_changed_lines() {
    let dir = temp_dir("air-tools-file-edit-max-changed-lines");
    fs::write(dir.join("note.txt"), "one\ntwo\nthree\n").unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "file.read": {
                  "kind": "file_read",
                  "capability": "file.read",
                  "base_dir": "."
                },
                "file.edit": {
                  "kind": "file_edit",
                  "capability": "file.write",
                  "base_dir": ".",
                  "max_changed_lines": 2
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();
    tools
        .call_tool("file.read", &json!({"path": "note.txt"}))
        .unwrap();

    let error = tools
        .call_tool(
            "file.edit",
            &json!({
                "path": "note.txt",
                "old_string": "one\ntwo\nthree\n",
                "new_string": "four\nfive\nsix\n"
            }),
        )
        .unwrap_err();

    assert!(error.to_string().contains("max_changed_lines=2"));
    assert_eq!(
        fs::read_to_string(dir.join("note.txt")).unwrap(),
        "one\ntwo\nthree\n"
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn file_edit_dry_run_checks_without_writing() {
    let dir = temp_dir("air-tools-file-edit-dry-run");
    fs::write(dir.join("note.txt"), "hello AIR\n").unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "file.read": {
                  "kind": "file_read",
                  "capability": "file.read",
                  "base_dir": "."
                },
                "file.edit": {
                  "kind": "file_edit",
                  "capability": "file.write",
                  "base_dir": ".",
                  "max_bytes": 1024
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();
    tools
        .call_tool("file.read", &json!({"path": "note.txt"}))
        .unwrap();

    let output = tools
        .call_tool(
            "file.edit",
            &json!({
                "path": "note.txt",
                "old_string": "AIR",
                "new_string": "agent IR",
                "dry_run": true
            }),
        )
        .unwrap();

    assert_eq!(
        fs::read_to_string(dir.join("note.txt")).unwrap(),
        "hello AIR\n"
    );
    assert_eq!(output["success"], json!(true));
    assert_eq!(output["checked"], json!(true));
    assert_eq!(output["applied"], json!(false));
    assert_eq!(output["file_count"], json!(1));
    assert_eq!(output["diagnostics"], json!([]));
    assert_eq!(output["files"][0]["path"], json!("note.txt"));
    assert_eq!(output["artifacts"][0]["metadata"]["dry_run"], json!(true));
    assert!(output["diff"].as_str().unwrap().contains("-AIR"));
    assert!(output["diff"].as_str().unwrap().contains("+agent IR"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn file_edit_applies_multiple_edits_after_read() {
    let dir = temp_dir("air-tools-file-edit-multiple-ops");
    fs::write(
        dir.join("note.txt"),
        "title: draft\nstatus: todo\nowner: unknown\n",
    )
    .unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "file.read": {
                  "kind": "file_read",
                  "capability": "file.read",
                  "base_dir": "."
                },
                "file.edit": {
                  "kind": "file_edit",
                  "capability": "file.write",
                  "base_dir": ".",
                  "max_bytes": 1024
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();
    tools
        .call_tool("file.read", &json!({"path": "note.txt"}))
        .unwrap();

    let output = tools
        .call_tool(
            "file.edit",
            &json!({
                "path": "note.txt",
                "edits": [
                    {
                        "old_string": "title: draft",
                        "new_string": "title: ready"
                    },
                    {
                        "old_string": "owner: unknown",
                        "new_string": "owner: agent"
                    }
                ]
            }),
        )
        .unwrap();

    assert_eq!(
        fs::read_to_string(dir.join("note.txt")).unwrap(),
        "title: ready\nstatus: todo\nowner: agent\n"
    );
    assert_eq!(output["edit_count"], json!(2));
    assert_eq!(output["replacements"], json!(2));
    assert_eq!(output["match_strategies"], json!(["exact", "exact"]));
    assert!(output["diff"].as_str().unwrap().contains("-title: draft"));
    assert!(output["diff"].as_str().unwrap().contains("+owner: agent"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn file_edit_multi_edit_is_atomic_when_later_edit_fails() {
    let dir = temp_dir("air-tools-file-edit-multiple-atomic");
    fs::write(dir.join("note.txt"), "alpha\nbeta\ngamma\n").unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "file.read": {
                  "kind": "file_read",
                  "capability": "file.read",
                  "base_dir": "."
                },
                "file.edit": {
                  "kind": "file_edit",
                  "capability": "file.write",
                  "base_dir": "."
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();
    tools
        .call_tool("file.read", &json!({"path": "note.txt"}))
        .unwrap();

    let error = tools
        .call_tool(
            "file.edit",
            &json!({
                "path": "note.txt",
                "edits": [
                    {
                        "old_string": "alpha",
                        "new_string": "ALPHA"
                    },
                    {
                        "old_string": "missing",
                        "new_string": "MISSING"
                    }
                ]
            }),
        )
        .unwrap_err();

    assert!(error
        .to_string()
        .contains("input.edits[1].old_string was not found"));
    assert_eq!(
        fs::read_to_string(dir.join("note.txt")).unwrap(),
        "alpha\nbeta\ngamma\n"
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn file_ops_dry_run_checks_edit_and_write_without_writing() {
    let dir = temp_dir("air-tools-file-ops-dry-run");
    fs::write(dir.join("note.txt"), "hello AIR\n").unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "file.read": {
                  "kind": "file_read",
                  "capability": "file.read",
                  "base_dir": "."
                },
                "file.ops": {
                  "kind": "file_ops",
                  "capability": "file.write",
                  "base_dir": ".",
                  "max_files": 4,
                  "max_bytes": 4096,
                  "allow_new_files": true
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();
    tools
        .call_tool("file.read", &json!({"path": "note.txt"}))
        .unwrap();

    let output = tools
        .call_tool(
            "file.ops",
            &json!({
                "dry_run": true,
                "operations": [
                    {
                        "kind": "edit",
                        "path": "note.txt",
                        "old_string": "AIR",
                        "new_string": "agent IR"
                    },
                    {
                        "kind": "write",
                        "path": "new.txt",
                        "content": "created by agent\n"
                    }
                ]
            }),
        )
        .unwrap();

    assert_eq!(
        fs::read_to_string(dir.join("note.txt")).unwrap(),
        "hello AIR\n"
    );
    assert!(!dir.join("new.txt").exists());
    assert_eq!(output["success"], json!(true));
    assert_eq!(output["checked"], json!(true));
    assert_eq!(output["applied"], json!(false));
    assert_eq!(output["file_count"], json!(2));
    assert_eq!(output["files"][0]["path"], json!("new.txt"));
    assert_eq!(output["files"][1]["path"], json!("note.txt"));
    assert!(output["diff"].as_str().unwrap().contains("+agent IR"));
    assert!(output["diff"]
        .as_str()
        .unwrap()
        .contains("+created by agent"));
    assert_eq!(output["artifacts"][0]["metadata"]["dry_run"], json!(true));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn file_ops_applies_operations_atomically() {
    let dir = temp_dir("air-tools-file-ops-apply");
    fs::write(dir.join("note.txt"), "hello AIR\n").unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "file.read": {
                  "kind": "file_read",
                  "capability": "file.read",
                  "base_dir": "."
                },
                "file.ops": {
                  "kind": "file_ops",
                  "capability": "file.write",
                  "base_dir": ".",
                  "max_files": 4,
                  "max_bytes": 4096,
                  "allow_new_files": true
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();
    tools
        .call_tool("file.read", &json!({"path": "note.txt"}))
        .unwrap();

    let output = tools
        .call_tool(
            "file.ops",
            &json!({
                "operations": [
                    {
                        "kind": "edit",
                        "path": "note.txt",
                        "old_string": "AIR",
                        "new_string": "agent IR"
                    },
                    {
                        "kind": "write",
                        "path": "new.txt",
                        "content": "created by agent\n"
                    }
                ]
            }),
        )
        .unwrap();

    assert_eq!(
        fs::read_to_string(dir.join("note.txt")).unwrap(),
        "hello agent IR\n"
    );
    assert_eq!(
        fs::read_to_string(dir.join("new.txt")).unwrap(),
        "created by agent\n"
    );
    assert_eq!(output["applied"], json!(true));
    assert_eq!(tools.tool_capability("file.ops"), Some("file.write"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn file_ops_rejects_paths_outside_allowed_paths() {
    let dir = temp_dir("air-tools-file-ops-allowed-paths");
    fs::write(dir.join("allowed.txt"), "allowed\n").unwrap();
    fs::write(dir.join("other.txt"), "other\n").unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "file.read": {
                  "kind": "file_read",
                  "capability": "file.read",
                  "base_dir": "."
                },
                "file.ops": {
                  "kind": "file_ops",
                  "capability": "file.write",
                  "base_dir": ".",
                  "require_read": true
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();
    tools
        .call_tool("file.read", &json!({"path": "other.txt"}))
        .unwrap();

    let error = tools
        .call_tool(
            "file.ops",
            &json!({
                "allowed_paths": ["allowed.txt", "another.txt"],
                "operations": [{
                    "kind": "edit",
                    "path": "other.txt",
                    "old_string": "other",
                    "new_string": "changed"
                }]
            }),
        )
        .unwrap_err();

    let error = error.to_string();
    assert!(error.contains("outside allowed_paths"));
    assert!(error.contains("allowed_paths=['allowed.txt', 'another.txt']"));
    assert_eq!(
        fs::read_to_string(dir.join("other.txt")).unwrap(),
        "other\n"
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn file_ops_repairs_single_allowed_path() {
    let dir = temp_dir("air-tools-file-ops-repairs-single-allowed-path");
    fs::write(dir.join("allowed.txt"), "allowed\n").unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "file.read": {
                  "kind": "file_read",
                  "capability": "file.read",
                  "base_dir": "."
                },
                "file.ops": {
                  "kind": "file_ops",
                  "capability": "file.write",
                  "base_dir": ".",
                  "require_read": true
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();
    tools
        .call_tool("file.read", &json!({"path": "allowed.txt"}))
        .unwrap();

    let output = tools
        .call_tool(
            "file.ops",
            &json!({
                "allowed_paths": ["allowed.txt"],
                "operations": [{
                    "kind": "edit",
                    "path": "allowed.",
                    "old_string": "allowed",
                    "new_string": "changed"
                }]
            }),
        )
        .unwrap();

    assert_eq!(
        fs::read_to_string(dir.join("allowed.txt")).unwrap(),
        "changed\n"
    );
    assert_eq!(output["path_repairs"][0]["from"], json!("allowed."));
    assert_eq!(output["path_repairs"][0]["to"], json!("allowed.txt"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn file_ops_rejects_diff_that_exceeds_max_changed_lines() {
    let dir = temp_dir("air-tools-file-ops-max-changed-lines");
    fs::write(dir.join("note.txt"), "one\ntwo\nthree\n").unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "file.read": {
                  "kind": "file_read",
                  "capability": "file.read",
                  "base_dir": "."
                },
                "file.ops": {
                  "kind": "file_ops",
                  "capability": "file.write",
                  "base_dir": ".",
                  "require_read": true,
                  "max_changed_lines": 2
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();
    tools
        .call_tool("file.read", &json!({"path": "note.txt"}))
        .unwrap();

    let error = tools
        .call_tool(
            "file.ops",
            &json!({
                "operations": [{
                    "kind": "replace_lines",
                    "path": "note.txt",
                    "start_line": 1,
                    "end_line": 3,
                    "lines": ["four", "five", "six"]
                }]
            }),
        )
        .unwrap_err();

    assert!(error.to_string().contains("max_changed_lines=2"));
    assert_eq!(
        fs::read_to_string(dir.join("note.txt")).unwrap(),
        "one\ntwo\nthree\n"
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn file_ops_rejects_diff_when_per_call_max_changed_lines_exceeds_tool_cap() {
    let dir = temp_dir("air-tools-file-ops-per-call-max-changed-lines-cap");
    fs::write(dir.join("note.txt"), "one\ntwo\nthree\n").unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "file.read": {
                  "kind": "file_read",
                  "capability": "file.read",
                  "base_dir": "."
                },
                "file.ops": {
                  "kind": "file_ops",
                  "capability": "file.write",
                  "base_dir": ".",
                  "require_read": true,
                  "max_changed_lines": 2
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();
    tools
        .call_tool("file.read", &json!({"path": "note.txt"}))
        .unwrap();

    let error = tools
        .call_tool(
            "file.ops",
            &json!({
                "max_changed_lines": 100,
                "operations": [{
                    "kind": "replace_lines",
                    "path": "note.txt",
                    "start_line": 1,
                    "end_line": 3,
                    "lines": ["four", "five", "six"]
                }]
            }),
        )
        .unwrap_err();

    assert!(error.to_string().contains("max_changed_lines=2"));
    assert_eq!(
        fs::read_to_string(dir.join("note.txt")).unwrap(),
        "one\ntwo\nthree\n"
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn file_ops_replace_lines_edits_large_files_without_full_file_payload() {
    let dir = temp_dir("air-tools-file-ops-replace-lines");
    let prefix = (0..200)
        .map(|index| format!("prefix-{index}\n"))
        .collect::<String>();
    let suffix = (0..200)
        .map(|index| format!("suffix-{index}\n"))
        .collect::<String>();
    fs::write(
        dir.join("large.txt"),
        format!("{prefix}old one\nold two\nold three\n{suffix}"),
    )
    .unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "file.read": {
                  "kind": "file_read",
                  "capability": "file.read",
                  "base_dir": ".",
                  "max_bytes": 8192
                },
                "file.ops": {
                  "kind": "file_ops",
                  "capability": "file.write",
                  "base_dir": ".",
                  "max_files": 2,
                  "max_bytes": 8192
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();
    tools
        .call_tool(
            "file.read",
            &json!({"path": "large.txt", "start_line": 200, "end_line": 205}),
        )
        .unwrap();

    let output = tools
        .call_tool(
            "file.ops",
            &json!({
                "operations": [{
                    "kind": "replace_lines",
                    "path": "large.txt",
                    "start_line": 201,
                    "end_line": 203,
                    "new_string": "new one\nnew two\n"
                }]
            }),
        )
        .unwrap();

    let updated = fs::read_to_string(dir.join("large.txt")).unwrap();
    assert_eq!(output["success"], json!(true));
    assert!(output["diff"].as_str().unwrap().contains("-old one"));
    assert!(output["diff"].as_str().unwrap().contains("+new one"));
    assert!(updated.contains("prefix-199\nnew one\nnew two\nsuffix-0"));
    assert!(!updated.contains("old two"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn file_ops_replace_lines_preserves_following_line_when_replacement_has_no_newline() {
    let dir = temp_dir("air-tools-file-ops-replace-lines-line-ending");
    fs::write(
        dir.join("math.js"),
        "function add(a, b) {\n  return a - b;\n}\n",
    )
    .unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "file.read": {
                  "kind": "file_read",
                  "capability": "file.read",
                  "base_dir": "."
                },
                "file.ops": {
                  "kind": "file_ops",
                  "capability": "file.write",
                  "base_dir": ".",
                  "max_files": 2,
                  "max_bytes": 4096
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();
    tools
        .call_tool("file.read", &json!({"path": "math.js"}))
        .unwrap();

    let output = tools
        .call_tool(
            "file.ops",
            &json!({
                "operations": [{
                    "kind": "replace_lines",
                    "path": "math.js",
                    "start_line": 2,
                    "end_line": 2,
                    "new_string": "  return a + b;"
                }]
            }),
        )
        .unwrap();

    assert_eq!(output["success"], json!(true));
    assert_eq!(
        fs::read_to_string(dir.join("math.js")).unwrap(),
        "function add(a, b) {\n  return a + b;\n}\n"
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn file_ops_accepts_single_operation_shorthand() {
    let dir = temp_dir("air-tools-file-ops-shorthand");
    fs::write(dir.join("note.txt"), "one\ntwo\nthree\n").unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "file.read": {
                  "kind": "file_read",
                  "capability": "file.read",
                  "base_dir": "."
                },
                "file.ops": {
                  "kind": "file_ops",
                  "capability": "file.write",
                  "base_dir": ".",
                  "max_files": 2,
                  "max_bytes": 4096
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();
    tools
        .call_tool("file.read", &json!({"path": "note.txt"}))
        .unwrap();

    let output = tools
        .call_tool(
            "file.ops",
            &json!({
                "kind": "replace_lines",
                "path": "note.txt",
                "args": {
                    "start_line": 2,
                    "end_line": 2,
                    "replacement": "TWO"
                }
            }),
        )
        .unwrap();

    assert_eq!(output["success"], json!(true));
    assert_eq!(
        fs::read_to_string(dir.join("note.txt")).unwrap(),
        "one\nTWO\nthree\n"
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn file_ops_accepts_ops_array_alias() {
    let dir = temp_dir("air-tools-file-ops-ops-alias");
    fs::write(dir.join("note.txt"), "one\ntwo\nthree\n").unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "file.read": {
                  "kind": "file_read",
                  "capability": "file.read",
                  "base_dir": "."
                },
                "file.ops": {
                  "kind": "file_ops",
                  "capability": "file.write",
                  "base_dir": ".",
                  "max_files": 2,
                  "max_bytes": 4096
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();
    tools
        .call_tool("file.read", &json!({"path": "note.txt"}))
        .unwrap();

    let output = tools
        .call_tool(
            "file.ops",
            &json!({
                "ops": [{
                    "kind": "replace_lines",
                    "path": "note.txt",
                    "start_line": 2,
                    "end_line": 2,
                    "content": "TWO"
                }]
            }),
        )
        .unwrap();

    assert_eq!(output["success"], json!(true));
    assert_eq!(
        fs::read_to_string(dir.join("note.txt")).unwrap(),
        "one\nTWO\nthree\n"
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn file_ops_applies_top_level_path_to_ops_alias_items() {
    let dir = temp_dir("air-tools-file-ops-top-level-path");
    fs::write(dir.join("note.txt"), "one\ntwo\nthree\n").unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "file.read": {
                  "kind": "file_read",
                  "capability": "file.read",
                  "base_dir": "."
                },
                "file.ops": {
                  "kind": "file_ops",
                  "capability": "file.write",
                  "base_dir": ".",
                  "max_files": 2,
                  "max_bytes": 4096
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();
    tools
        .call_tool("file.read", &json!({"path": "note.txt"}))
        .unwrap();

    let output = tools
        .call_tool(
            "file.ops",
            &json!({
                "path": "note.txt",
                "ops": [{
                    "kind": "replace_lines",
                    "start_line": 2,
                    "end_line": 2,
                    "content": "TWO"
                }]
            }),
        )
        .unwrap();

    assert_eq!(output["success"], json!(true));
    assert_eq!(
        fs::read_to_string(dir.join("note.txt")).unwrap(),
        "one\nTWO\nthree\n"
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn file_ops_applies_top_level_kind_and_path_to_edits_alias_items() {
    let dir = temp_dir("air-tools-file-ops-edits-alias");
    fs::write(dir.join("note.txt"), "one\ntwo\nthree\n").unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "file.read": {
                  "kind": "file_read",
                  "capability": "file.read",
                  "base_dir": "."
                },
                "file.ops": {
                  "kind": "file_ops",
                  "capability": "file.write",
                  "base_dir": ".",
                  "max_files": 2,
                  "max_bytes": 4096
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();
    tools
        .call_tool("file.read", &json!({"path": "note.txt"}))
        .unwrap();

    let output = tools
        .call_tool(
            "file.ops",
            &json!({
                "kind": "replace_lines",
                "path": "note.txt",
                "edits": [{
                    "start_line": 2,
                    "end_line": 2,
                    "new_lines": ["TWO"]
                }]
            }),
        )
        .unwrap();

    assert_eq!(output["success"], json!(true));
    assert_eq!(
        fs::read_to_string(dir.join("note.txt")).unwrap(),
        "one\nTWO\nthree\n"
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn file_ops_accepts_replace_lines_new_lines_alias() {
    let dir = temp_dir("air-tools-file-ops-new-lines-alias");
    fs::write(dir.join("note.txt"), "one\ntwo\nthree\n").unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "file.read": {
                  "kind": "file_read",
                  "capability": "file.read",
                  "base_dir": "."
                },
                "file.ops": {
                  "kind": "file_ops",
                  "capability": "file.write",
                  "base_dir": ".",
                  "max_files": 2,
                  "max_bytes": 4096
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();
    tools
        .call_tool("file.read", &json!({"path": "note.txt"}))
        .unwrap();

    let output = tools
        .call_tool(
            "file.ops",
            &json!({
                "ops": [{
                    "kind": "replace_lines",
                    "path": "note.txt",
                    "start_line": 2,
                    "end_line": 2,
                    "new_lines": ["TWO"]
                }]
            }),
        )
        .unwrap();

    assert_eq!(output["success"], json!(true));
    assert_eq!(
        fs::read_to_string(dir.join("note.txt")).unwrap(),
        "one\nTWO\nthree\n"
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn file_ops_accepts_replace_lines_lines_alias() {
    let dir = temp_dir("air-tools-file-ops-lines-alias");
    fs::write(dir.join("note.txt"), "one\ntwo\nthree\n").unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "file.read": {
                  "kind": "file_read",
                  "capability": "file.read",
                  "base_dir": "."
                },
                "file.ops": {
                  "kind": "file_ops",
                  "capability": "file.write",
                  "base_dir": ".",
                  "max_files": 2,
                  "max_bytes": 4096
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();
    tools
        .call_tool("file.read", &json!({"path": "note.txt"}))
        .unwrap();

    let output = tools
        .call_tool(
            "file.ops",
            &json!({
                "ops": [{
                    "kind": "replace_lines",
                    "path": "note.txt",
                    "start_line": 2,
                    "end_line": 2,
                    "lines": ["TWO"]
                }]
            }),
        )
        .unwrap();

    assert_eq!(output["success"], json!(true));
    assert_eq!(
        fs::read_to_string(dir.join("note.txt")).unwrap(),
        "one\nTWO\nthree\n"
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn file_ops_replace_lines_rejects_invalid_ranges_atomically() {
    let dir = temp_dir("air-tools-file-ops-replace-lines-invalid");
    fs::write(dir.join("note.txt"), "one\ntwo\n").unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "file.read": {
                  "kind": "file_read",
                  "capability": "file.read",
                  "base_dir": "."
                },
                "file.ops": {
                  "kind": "file_ops",
                  "capability": "file.write",
                  "base_dir": ".",
                  "max_files": 2,
                  "max_bytes": 4096
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();
    tools
        .call_tool("file.read", &json!({"path": "note.txt"}))
        .unwrap();

    let output = tools
        .call_tool(
            "file.ops",
            &json!({
                "operations": [{
                    "kind": "replace_lines",
                    "path": "note.txt",
                    "start_line": 2,
                    "end_line": 5,
                    "new_string": "replacement\n"
                }]
            }),
        )
        .unwrap();

    assert_eq!(output["success"], json!(false));
    assert_eq!(output["diagnostics"][0]["kind"], json!("replace_lines"));
    assert_eq!(
        fs::read_to_string(dir.join("note.txt")).unwrap(),
        "one\ntwo\n"
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn file_ops_is_atomic_when_later_operation_fails() {
    let dir = temp_dir("air-tools-file-ops-atomic");
    fs::write(dir.join("note.txt"), "hello AIR\n").unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "file.read": {
                  "kind": "file_read",
                  "capability": "file.read",
                  "base_dir": "."
                },
                "file.ops": {
                  "kind": "file_ops",
                  "capability": "file.write",
                  "base_dir": ".",
                  "max_files": 4,
                  "max_bytes": 4096,
                  "allow_new_files": true
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();
    tools
        .call_tool("file.read", &json!({"path": "note.txt"}))
        .unwrap();

    let output = tools
        .call_tool(
            "file.ops",
            &json!({
                "operations": [
                    {
                        "kind": "edit",
                        "path": "note.txt",
                        "old_string": "AIR",
                        "new_string": "agent IR"
                    },
                    {
                        "kind": "edit",
                        "path": "note.txt",
                        "old_string": "missing",
                        "new_string": "MISSING"
                    }
                ]
            }),
        )
        .unwrap();

    assert_eq!(output["success"], json!(false));
    assert_eq!(output["checked"], json!(true));
    assert_eq!(output["applied"], json!(false));
    assert!(output["diagnostics"][0]["message"]
        .as_str()
        .unwrap()
        .contains("input.operations[1].old_string was not found"));
    assert_eq!(output["diagnostics"][0]["operation_index"], json!(1));
    assert_eq!(
        output["diagnostics"][0]["operation_label"],
        json!("input.operations[1]")
    );
    assert_eq!(output["diagnostics"][0]["path"], json!("note.txt"));
    assert_eq!(output["diagnostics"][0]["kind"], json!("edit"));
    assert_eq!(output["diagnostics"][0]["field"], json!("old_string"));
    assert_eq!(output["diagnostics"][0]["match_strategy"], json!("auto"));
    assert_eq!(output["diagnostics"][0]["match_count"], json!(0));
    assert_eq!(
        fs::read_to_string(dir.join("note.txt")).unwrap(),
        "hello AIR\n"
    );
    assert!(!dir.join("new.txt").exists());
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn file_ops_old_string_not_found_reports_anchor_line() {
    let dir = temp_dir("air-tools-file-ops-anchor-line");
    fs::write(dir.join("note.txt"), "alpha\nbeta\ngamma\n").unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "file.read": {
                  "kind": "file_read",
                  "capability": "file.read",
                  "base_dir": "."
                },
                "file.ops": {
                  "kind": "file_ops",
                  "capability": "file.write",
                  "base_dir": ".",
                  "max_bytes": 4096
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();
    tools
        .call_tool("file.read", &json!({"path": "note.txt"}))
        .unwrap();

    let output = tools
        .call_tool(
            "file.ops",
            &json!({
                "operations": [{
                    "kind": "edit",
                    "path": "note.txt",
                    "old_string": "wrong beta\ngamma",
                    "new_string": "fixed beta\ngamma"
                }]
            }),
        )
        .unwrap();

    assert_eq!(output["success"], json!(false));
    assert_eq!(output["diagnostics"][0]["field"], json!("old_string"));
    assert_eq!(output["diagnostics"][0]["match_count"], json!(0));
    assert_eq!(output["diagnostics"][0]["line"], json!(3));
    assert_eq!(
        output["diagnostics"][0]["line_source"],
        json!("old_string_anchor")
    );
    assert_eq!(
        fs::read_to_string(dir.join("note.txt")).unwrap(),
        "alpha\nbeta\ngamma\n"
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn file_ops_existing_write_without_overwrite_returns_structured_diagnostic() {
    let dir = temp_dir("air-tools-file-ops-write-existing-diagnostic");
    fs::write(dir.join("note.txt"), "hello AIR\n").unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "file.read": {
                  "kind": "file_read",
                  "capability": "file.read",
                  "base_dir": "."
                },
                "file.ops": {
                  "kind": "file_ops",
                  "capability": "file.write",
                  "base_dir": ".",
                  "max_files": 2,
                  "max_bytes": 4096,
                  "allow_new_files": true,
                  "allow_overwrite": false
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();
    tools
        .call_tool("file.read", &json!({"path": "note.txt"}))
        .unwrap();

    let output = tools
        .call_tool(
            "file.ops",
            &json!({
                "operations": [{
                    "kind": "write",
                    "path": "note.txt",
                    "content": "replacement\n"
                }]
            }),
        )
        .unwrap();

    assert_eq!(output["success"], json!(false));
    assert_eq!(output["diagnostics"][0]["kind"], json!("write"));
    assert_eq!(output["diagnostics"][0]["field"], json!("path"));
    assert!(output["diagnostics"][0]["message"]
        .as_str()
        .unwrap()
        .contains("use kind=edit or kind=replace_lines"));
    assert_eq!(
        fs::read_to_string(dir.join("note.txt")).unwrap(),
        "hello AIR\n"
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn file_ops_defaults_to_auto_match_strategy() {
    let dir = temp_dir("air-tools-file-ops-auto-match");
    fs::write(
        dir.join("note.txt"),
        "function demo() {\n    return \"before\";\n}\n",
    )
    .unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "file.read": {
                  "kind": "file_read",
                  "capability": "file.read",
                  "base_dir": "."
                },
                "file.ops": {
                  "kind": "file_ops",
                  "capability": "file.write",
                  "base_dir": ".",
                  "max_files": 4,
                  "max_bytes": 4096
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();
    tools
        .call_tool("file.read", &json!({"path": "note.txt"}))
        .unwrap();

    let output = tools
        .call_tool(
            "file.ops",
            &json!({
                "operations": [
                    {
                        "kind": "edit",
                        "path": "note.txt",
                        "old_string": "function demo() {\nreturn \"before\";\n}",
                        "new_string": "function demo() {\n    return \"after\";\n}"
                    }
                ]
            }),
        )
        .unwrap();

    assert_eq!(output["success"], json!(true));
    assert_eq!(output["match_strategies"], json!(["line_trimmed"]));
    assert_eq!(
        fs::read_to_string(dir.join("note.txt")).unwrap(),
        "function demo() {\n    return \"after\";\n}\n"
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn file_ops_respects_explicit_exact_match_strategy() {
    let dir = temp_dir("air-tools-file-ops-exact-match");
    fs::write(
        dir.join("note.txt"),
        "function demo() {\n    return \"before\";\n}\n",
    )
    .unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "file.read": {
                  "kind": "file_read",
                  "capability": "file.read",
                  "base_dir": "."
                },
                "file.ops": {
                  "kind": "file_ops",
                  "capability": "file.write",
                  "base_dir": ".",
                  "max_files": 4,
                  "max_bytes": 4096
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();
    tools
        .call_tool("file.read", &json!({"path": "note.txt"}))
        .unwrap();

    let output = tools
        .call_tool(
            "file.ops",
            &json!({
                "operations": [
                    {
                        "kind": "edit",
                        "path": "note.txt",
                        "old_string": "function demo() {\nreturn \"before\";\n}",
                        "new_string": "function demo() {\n    return \"after\";\n}",
                        "match_strategy": "exact"
                    }
                ]
            }),
        )
        .unwrap();

    assert_eq!(output["success"], json!(false));
    assert!(output["diagnostics"][0]["message"]
        .as_str()
        .unwrap()
        .contains("match_strategy=exact"));
    assert_eq!(
        fs::read_to_string(dir.join("note.txt")).unwrap(),
        "function demo() {\n    return \"before\";\n}\n"
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn file_edit_rejects_stale_read() {
    let dir = temp_dir("air-tools-file-edit-stale-read");
    fs::write(dir.join("note.txt"), "hello AIR\n").unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "file.read": {
                  "kind": "file_read",
                  "capability": "file.read",
                  "base_dir": "."
                },
                "file.edit": {
                  "kind": "file_edit",
                  "capability": "file.write",
                  "base_dir": "."
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();
    tools
        .call_tool("file.read", &json!({"path": "note.txt"}))
        .unwrap();
    std::thread::sleep(Duration::from_millis(20));
    fs::write(dir.join("note.txt"), "hello outside\n").unwrap();

    let error = tools
        .call_tool(
            "file.edit",
            &json!({"path": "note.txt", "old_string": "outside", "new_string": "agent"}),
        )
        .unwrap_err();

    assert!(error
        .to_string()
        .contains("modified after it was last read"));
    assert_eq!(
        fs::read_to_string(dir.join("note.txt")).unwrap(),
        "hello outside\n"
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn file_edit_supports_explicit_line_trimmed_match_strategy() {
    let dir = temp_dir("air-tools-file-edit-line-trimmed");
    fs::write(
        dir.join("note.txt"),
        "function demo() {\n    return \"before\";\n}\n",
    )
    .unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "file.read": {
                  "kind": "file_read",
                  "capability": "file.read",
                  "base_dir": "."
                },
                "file.edit": {
                  "kind": "file_edit",
                  "capability": "file.write",
                  "base_dir": "."
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();
    tools
        .call_tool("file.read", &json!({"path": "note.txt"}))
        .unwrap();

    let exact_error = tools
        .call_tool(
            "file.edit",
            &json!({
                "path": "note.txt",
                "old_string": "function demo() {\nreturn \"before\";\n}",
                "new_string": "function demo() {\n    return \"after\";\n}"
            }),
        )
        .unwrap_err();
    assert!(exact_error.to_string().contains("match_strategy=exact"));

    let output = tools
        .call_tool(
            "file.edit",
            &json!({
                "path": "note.txt",
                "old_string": "function demo() {\nreturn \"before\";\n}",
                "new_string": "function demo() {\n    return \"after\";\n}",
                "match_strategy": "line_trimmed"
            }),
        )
        .unwrap();

    assert_eq!(output["match_strategy"], json!("line_trimmed"));
    assert_eq!(
        fs::read_to_string(dir.join("note.txt")).unwrap(),
        "function demo() {\n    return \"after\";\n}\n"
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn file_edit_rejects_unknown_match_strategy() {
    let dir = temp_dir("air-tools-file-edit-match-strategy");
    fs::write(dir.join("note.txt"), "hello AIR\n").unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "file.read": {
                  "kind": "file_read",
                  "capability": "file.read",
                  "base_dir": "."
                },
                "file.edit": {
                  "kind": "file_edit",
                  "capability": "file.write",
                  "base_dir": "."
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();
    tools
        .call_tool("file.read", &json!({"path": "note.txt"}))
        .unwrap();

    let error = tools
        .call_tool(
            "file.edit",
            &json!({
                "path": "note.txt",
                "old_string": "AIR",
                "new_string": "Agent",
                "match_strategy": "semantic_guess"
            }),
        )
        .unwrap_err();

    assert!(error.to_string().contains("input.match_strategy"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn file_edit_rejects_noop_replacement() {
    let dir = temp_dir("air-tools-file-edit-noop");
    fs::write(dir.join("note.txt"), "hello AIR\n").unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "file.read": {
                  "kind": "file_read",
                  "capability": "file.read",
                  "base_dir": "."
                },
                "file.edit": {
                  "kind": "file_edit",
                  "capability": "file.write",
                  "base_dir": "."
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();
    tools
        .call_tool("file.read", &json!({"path": "note.txt"}))
        .unwrap();

    let error = tools
        .call_tool(
            "file.edit",
            &json!({"path": "note.txt", "old_string": "AIR", "new_string": "AIR"}),
        )
        .unwrap_err();

    assert!(error.to_string().contains("must be different"));
    assert_eq!(
        fs::read_to_string(dir.join("note.txt")).unwrap(),
        "hello AIR\n"
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn file_edit_rejects_multiple_matches_without_replace_all() {
    let dir = temp_dir("air-tools-file-edit-multiple");
    fs::write(dir.join("note.txt"), "AIR AIR\n").unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "file.read": {
                  "kind": "file_read",
                  "capability": "file.read",
                  "base_dir": "."
                },
                "file.edit": {
                  "kind": "file_edit",
                  "capability": "file.write",
                  "base_dir": ".",
                  "allow_replace_all": true
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();
    tools
        .call_tool("file.read", &json!({"path": "note.txt"}))
        .unwrap();

    let error = tools
        .call_tool(
            "file.edit",
            &json!({"path": "note.txt", "old_string": "AIR", "new_string": "Agent"}),
        )
        .unwrap_err();
    assert!(error.to_string().contains("matched 2 times"));

    let output = tools
        .call_tool(
            "file.edit",
            &json!({
                "path": "note.txt",
                "old_string": "AIR",
                "new_string": "Agent",
                "replace_all": true
            }),
        )
        .unwrap();
    assert_eq!(output["replacements"], json!(2));
    assert_eq!(
        fs::read_to_string(dir.join("note.txt")).unwrap(),
        "Agent Agent\n"
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn file_patch_applies_unified_diff_after_read() {
    let dir = temp_dir("air-tools-file-patch");
    fs::write(dir.join("note.txt"), "before\n").unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "file.read": {
                  "kind": "file_read",
                  "capability": "file.read",
                  "base_dir": "."
                },
                "file.patch": {
                  "kind": "file_patch",
                  "capability": "file.write",
                  "repo_dir": ".",
                  "require_read": true,
                  "max_files": 3
                }
              }
            }"#,
    );
    let patch = "diff --git a/note.txt b/note.txt\n--- a/note.txt\n+++ b/note.txt\n@@ -1 +1 @@\n-before\n+after\n";
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let error = tools
        .call_tool("file.patch", &json!({"patch": patch}))
        .unwrap_err();
    assert!(error.to_string().contains("must be read before patch"));

    tools
        .call_tool("file.read", &json!({"path": "note.txt"}))
        .unwrap();
    let output = tools
        .call_tool("file.patch", &json!({"patch": patch}))
        .unwrap();

    assert_eq!(fs::read_to_string(dir.join("note.txt")).unwrap(), "after\n");
    assert_eq!(output["file_count"], json!(1));
    assert_eq!(output["artifacts"][0]["kind"], json!("file_patch"));
    assert_eq!(tools.tool_capability("file.patch"), Some("file.write"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn file_patch_rejects_paths_outside_allowed_paths() {
    let dir = temp_dir("air-tools-file-patch-allowed-paths");
    fs::write(dir.join("allowed.txt"), "allowed\n").unwrap();
    fs::write(dir.join("other.txt"), "other\n").unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "file.read": {
                  "kind": "file_read",
                  "capability": "file.read",
                  "base_dir": "."
                },
                "file.patch": {
                  "kind": "file_patch",
                  "capability": "file.write",
                  "repo_dir": ".",
                  "require_read": true
                }
              }
            }"#,
    );
    let patch = "diff --git a/other.txt b/other.txt\n--- a/other.txt\n+++ b/other.txt\n@@ -1 +1 @@\n-other\n+changed\n";
    let mut tools = ConfigTools::from_file(config_path).unwrap();
    tools
        .call_tool("file.read", &json!({"path": "other.txt"}))
        .unwrap();

    let error = tools
        .call_tool(
            "file.patch",
            &json!({"patch": patch, "allowed_paths": ["allowed.txt"]}),
        )
        .unwrap_err();

    let error = error.to_string();
    assert!(error.contains("outside allowed_paths"));
    assert!(error.contains("allowed_paths=['allowed.txt']"));
    assert_eq!(
        fs::read_to_string(dir.join("other.txt")).unwrap(),
        "other\n"
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn file_patch_rejects_diff_that_exceeds_max_changed_lines() {
    let dir = temp_dir("air-tools-file-patch-max-changed-lines");
    fs::write(dir.join("note.txt"), "one\ntwo\nthree\n").unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "file.read": {
                  "kind": "file_read",
                  "capability": "file.read",
                  "base_dir": "."
                },
                "file.patch": {
                  "kind": "file_patch",
                  "capability": "file.write",
                  "repo_dir": ".",
                  "require_read": true,
                  "max_changed_lines": 2
                }
              }
            }"#,
    );
    let patch = "diff --git a/note.txt b/note.txt\n--- a/note.txt\n+++ b/note.txt\n@@ -1,3 +1,3 @@\n-one\n-two\n-three\n+four\n+five\n+six\n";
    let mut tools = ConfigTools::from_file(config_path).unwrap();
    tools
        .call_tool("file.read", &json!({"path": "note.txt"}))
        .unwrap();

    let error = tools
        .call_tool("file.patch", &json!({"patch": patch}))
        .unwrap_err();

    assert!(error.to_string().contains("max_changed_lines=2"));
    assert_eq!(
        fs::read_to_string(dir.join("note.txt")).unwrap(),
        "one\ntwo\nthree\n"
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn file_patch_rejects_stale_read() {
    let dir = temp_dir("air-tools-file-patch-stale-read");
    fs::write(dir.join("note.txt"), "before\n").unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "file.read": {
                  "kind": "file_read",
                  "capability": "file.read",
                  "base_dir": "."
                },
                "file.patch": {
                  "kind": "file_patch",
                  "capability": "file.write",
                  "repo_dir": ".",
                  "require_read": true
                }
              }
            }"#,
    );
    let patch = "diff --git a/note.txt b/note.txt\n--- a/note.txt\n+++ b/note.txt\n@@ -1 +1 @@\n-outside\n+after\n";
    let mut tools = ConfigTools::from_file(config_path).unwrap();
    tools
        .call_tool("file.read", &json!({"path": "note.txt"}))
        .unwrap();
    std::thread::sleep(Duration::from_millis(20));
    fs::write(dir.join("note.txt"), "outside\n").unwrap();

    let error = tools
        .call_tool("file.patch", &json!({"patch": patch}))
        .unwrap_err();

    assert!(error
        .to_string()
        .contains("modified after it was last read"));
    assert_eq!(
        fs::read_to_string(dir.join("note.txt")).unwrap(),
        "outside\n"
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn file_patch_can_create_new_file_when_allowed() {
    let dir = temp_dir("air-tools-file-patch-new");
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "file.patch": {
                  "kind": "file_patch",
                  "capability": "file.write",
                  "repo_dir": ".",
                  "allow_new_files": true
                }
              }
            }"#,
    );
    let patch = "diff --git a/new.txt b/new.txt\nnew file mode 100644\n--- /dev/null\n+++ b/new.txt\n@@ -0,0 +1 @@\n+hello\n";
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let output = tools
        .call_tool("file.patch", &json!({"patch": patch}))
        .unwrap();

    assert_eq!(fs::read_to_string(dir.join("new.txt")).unwrap(), "hello\n");
    assert_eq!(output["files"][0]["kind"], json!("new"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn file_patch_dry_run_returns_failed_check_without_applying() {
    let dir = temp_dir("air-tools-file-patch-dry-run");
    fs::write(dir.join("note.txt"), "before\n").unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "file.read": {
                  "kind": "file_read",
                  "capability": "file.read",
                  "base_dir": "."
                },
                "file.patch": {
                  "kind": "file_patch",
                  "capability": "file.write",
                  "repo_dir": ".",
                  "require_read": true
                }
              }
            }"#,
    );
    let patch = "diff --git a/note.txt b/note.txt\n--- a/note.txt\n+++ b/note.txt\n@@ -1 +1 @@\n-not-present\n+after\n";
    let mut tools = ConfigTools::from_file(config_path).unwrap();
    tools
        .call_tool("file.read", &json!({"path": "note.txt"}))
        .unwrap();

    let output = tools
        .call_tool("file.patch", &json!({"patch": patch, "dry_run": true}))
        .unwrap();

    assert_eq!(output["success"], json!(false));
    assert_eq!(output["checked"], json!(true));
    assert_eq!(output["applied"], json!(false));
    assert_eq!(output["diagnostics"][0]["source"], json!("file_patch"));
    assert_eq!(
        fs::read_to_string(dir.join("note.txt")).unwrap(),
        "before\n"
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn file_patch_rejects_parent_path_escape() {
    let dir = temp_dir("air-tools-file-patch-boundary");
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "file.patch": {
                  "kind": "file_patch",
                  "capability": "file.write",
                  "repo_dir": ".",
                  "require_read": false
                }
              }
            }"#,
    );
    let patch = "diff --git a/../escape.txt b/../escape.txt\n--- a/../escape.txt\n+++ b/../escape.txt\n@@ -1 +1 @@\n-before\n+after\n";
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let error = tools
        .call_tool("file.patch", &json!({"patch": patch}))
        .unwrap_err();

    assert!(error.to_string().contains("relative paths inside repo_dir"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn file_read_can_return_line_ranges() {
    let dir = temp_dir("air-tools-file-read-range");
    fs::write(dir.join("note.txt"), "one\ntwo\nthree\nfour\n").unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "file.read": {
                  "kind": "file_read",
                  "capability": "file.read",
                  "base_dir": "."
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let output = tools
        .call_tool(
            "file.read",
            &json!({"path": "note.txt", "start_line": 2, "end_line": 3}),
        )
        .unwrap();

    assert_eq!(output["content"], json!("two\nthree"));
    assert_eq!(output["start_line"], json!(2));
    assert_eq!(output["end_line"], json!(3));
    assert_eq!(output["total_lines"], json!(4));
    assert_eq!(output["artifacts"][0]["metadata"]["start_line"], json!(2));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn file_read_can_return_context_around_contains_match() {
    let dir = temp_dir("air-tools-file-read-contains");
    fs::write(
        dir.join("note.txt"),
        "alpha\nbefore\ntarget symbol\nafter\nomega\n",
    )
    .unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "file.read": {
                  "kind": "file_read",
                  "capability": "file.read",
                  "base_dir": "."
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let output = tools
        .call_tool(
            "file.read",
            &json!({
                "path": "note.txt",
                "contains": "target",
                "context_lines": 1,
                "line_numbers": true
            }),
        )
        .unwrap();

    assert_eq!(output["content"], json!("before\ntarget symbol\nafter"));
    assert_eq!(output["start_line"], json!(2));
    assert_eq!(output["end_line"], json!(4));
    assert_eq!(output["match_line"], json!(3));
    assert_eq!(
        output["numbered_content"],
        json!("00002| before\n00003| target symbol\n00004| after")
    );
    assert_eq!(
        output["artifacts"][0]["metadata"]["contains"],
        json!("target")
    );
    assert_eq!(output["occurrence"], json!(1));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn file_read_can_return_context_around_later_contains_occurrence() {
    let dir = temp_dir("air-tools-file-read-contains-occurrence");
    fs::write(
        dir.join("note.txt"),
        "target first\nmiddle\nbefore\ntarget second\nafter\n",
    )
    .unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "file.read": {
                  "kind": "file_read",
                  "capability": "file.read",
                  "base_dir": "."
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let output = tools
        .call_tool(
            "file.read",
            &json!({
                "path": "note.txt",
                "contains": "target",
                "occurrence": 2,
                "context_lines": 1
            }),
        )
        .unwrap();

    assert_eq!(output["content"], json!("before\ntarget second\nafter"));
    assert_eq!(output["match_line"], json!(4));
    assert_eq!(output["occurrence"], json!(2));
    assert_eq!(output["artifacts"][0]["metadata"]["occurrence"], json!(2));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn file_read_rejects_missing_contains_without_returning_wrong_context() {
    let dir = temp_dir("air-tools-file-read-contains-missing");
    fs::write(dir.join("note.txt"), "alpha\nbeta\n").unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "file.read": {
                  "kind": "file_read",
                  "capability": "file.read",
                  "base_dir": "."
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let error = tools
        .call_tool(
            "file.read",
            &json!({"path": "note.txt", "contains": "gamma"}),
        )
        .unwrap_err();

    assert!(error
        .to_string()
        .contains("input.contains occurrence=1 was not found"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn file_read_rejects_missing_contains_occurrence() {
    let dir = temp_dir("air-tools-file-read-contains-occurrence-missing");
    fs::write(dir.join("note.txt"), "target once\n").unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "file.read": {
                  "kind": "file_read",
                  "capability": "file.read",
                  "base_dir": "."
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let error = tools
        .call_tool(
            "file.read",
            &json!({"path": "note.txt", "contains": "target", "occurrence": 2}),
        )
        .unwrap_err();

    assert!(error
        .to_string()
        .contains("input.contains occurrence=2 was not found"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn git_diff_returns_diff_for_configured_repo() {
    let dir = temp_dir("air-tools-git-diff");
    Command::new("git")
        .arg("-C")
        .arg(&dir)
        .arg("init")
        .output()
        .unwrap();
    fs::write(dir.join("note.txt"), "before\n").unwrap();
    Command::new("git")
        .arg("-C")
        .arg(&dir)
        .args(["add", "note.txt"])
        .output()
        .unwrap();
    Command::new("git")
        .arg("-C")
        .arg(&dir)
        .args(["commit", "-m", "init"])
        .env("GIT_AUTHOR_NAME", "AIR")
        .env("GIT_AUTHOR_EMAIL", "air@example.com")
        .env("GIT_COMMITTER_NAME", "AIR")
        .env("GIT_COMMITTER_EMAIL", "air@example.com")
        .output()
        .unwrap();
    fs::write(dir.join("note.txt"), "after\n").unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "git.diff": {
                  "kind": "git_diff",
                  "capability": "code.read",
                  "repo_dir": "."
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let output = tools
        .call_tool("git.diff", &json!({"path": "note.txt"}))
        .unwrap();

    assert!(output["diff"].as_str().unwrap().contains("-before"));
    assert!(output["diff"].as_str().unwrap().contains("+after"));
    assert_eq!(output["artifacts"][0]["kind"], json!("git_diff"));
    assert!(output["artifacts"][0]["id"]
        .as_str()
        .unwrap()
        .contains("note.txt"));
    assert_eq!(tools.tool_capability("git.diff"), Some("code.read"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn git_diff_accepts_patch_file_objects_and_empty_filters() {
    let dir = temp_dir("air-tools-git-diff-files");
    Command::new("git")
        .arg("-C")
        .arg(&dir)
        .arg("init")
        .output()
        .unwrap();
    fs::write(dir.join("agent.txt"), "before\n").unwrap();
    fs::write(dir.join("user.txt"), "keep\n").unwrap();
    Command::new("git")
        .arg("-C")
        .arg(&dir)
        .args(["add", "agent.txt", "user.txt"])
        .output()
        .unwrap();
    Command::new("git")
        .arg("-C")
        .arg(&dir)
        .args(["commit", "-m", "init"])
        .env("GIT_AUTHOR_NAME", "AIR")
        .env("GIT_AUTHOR_EMAIL", "air@example.com")
        .env("GIT_COMMITTER_NAME", "AIR")
        .env("GIT_COMMITTER_EMAIL", "air@example.com")
        .output()
        .unwrap();
    fs::write(dir.join("agent.txt"), "after\n").unwrap();
    fs::write(dir.join("user.txt"), "dirty user change\n").unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "git.diff": {
                  "kind": "git_diff",
                  "repo_dir": "."
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let output = tools
        .call_tool(
            "git.diff",
            &json!({"files": [{"path": "agent.txt", "kind": "modify"}]}),
        )
        .unwrap();

    let diff = output["diff"].as_str().unwrap();
    assert!(diff.contains("agent.txt"));
    assert!(diff.contains("+after"));
    assert!(!diff.contains("user.txt"));

    let empty = tools.call_tool("git.diff", &json!({"files": []})).unwrap();
    assert_eq!(empty["diff"], json!(""));
    assert_eq!(empty["bytes"], json!(0));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn git_diff_rejects_parent_path_filters() {
    let dir = temp_dir("air-tools-git-diff-path");
    Command::new("git")
        .arg("-C")
        .arg(&dir)
        .arg("init")
        .output()
        .unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "git.diff": {
                  "kind": "git_diff",
                  "capability": "code.read",
                  "repo_dir": "."
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let error = tools
        .call_tool("git.diff", &json!({"path": "../outside"}))
        .unwrap_err();

    assert!(error.to_string().contains("relative paths inside repo_dir"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn git_diff_includes_requested_untracked_text_file() {
    let dir = temp_dir("air-tools-git-diff-untracked");
    Command::new("git")
        .arg("-C")
        .arg(&dir)
        .arg("init")
        .output()
        .unwrap();
    fs::write(dir.join("tracked.txt"), "before\n").unwrap();
    Command::new("git")
        .arg("-C")
        .arg(&dir)
        .args(["add", "tracked.txt"])
        .output()
        .unwrap();
    fs::write(dir.join("new.txt"), "new\ncontent\n").unwrap();
    fs::write(dir.join("ignored.txt"), "not requested\n").unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "git.diff": {
                  "kind": "git_diff",
                  "capability": "code.read",
                  "repo_dir": "."
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let output = tools
        .call_tool("git.diff", &json!({"path": "new.txt"}))
        .unwrap();

    let diff = output["diff"].as_str().unwrap();
    assert!(diff.contains("diff --git a/new.txt b/new.txt"));
    assert!(diff.contains("new file mode 100644"));
    assert!(diff.contains("+new"));
    assert!(diff.contains("+content"));
    assert!(!diff.contains("ignored.txt"));
    assert_eq!(
        output["artifacts"][0]["metadata"]["untracked_files"],
        json!(["new.txt"])
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn git_status_returns_structured_workspace_entries() {
    let dir = temp_dir("air-tools-git-status");
    Command::new("git")
        .arg("-C")
        .arg(&dir)
        .arg("init")
        .output()
        .unwrap();
    fs::write(dir.join("tracked.txt"), "before\n").unwrap();
    Command::new("git")
        .arg("-C")
        .arg(&dir)
        .args(["add", "tracked.txt"])
        .output()
        .unwrap();
    fs::write(dir.join("tracked.txt"), "after\n").unwrap();
    fs::write(dir.join("new.txt"), "new\n").unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "git.status": {
                  "kind": "git_status",
                  "capability": "code.read",
                  "repo_dir": ".",
                  "max_files": 10
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let output = tools.call_tool("git.status", &json!({})).unwrap();

    assert_eq!(output["clean"], json!(false));
    assert!(output["file_count"].as_u64().unwrap() >= 2);
    assert!(output["entries"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry["path"] == json!("tracked.txt")));
    assert!(output["entries"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry["path"] == json!("new.txt") && entry["status"] == json!("untracked")));
    assert_eq!(output["artifacts"][0]["kind"], json!("git_status"));
    assert_eq!(tools.tool_capability("git.status"), Some("code.read"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn repo_files_lists_and_filters_repo_paths() {
    let dir = temp_dir("air-tools-repo-files");
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(dir.join("src/lib.rs"), "pub fn alpha() {}\n").unwrap();
    fs::write(dir.join("README.md"), "alpha docs\n").unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "repo.files": {
                  "kind": "repo_files",
                  "capability": "code.read",
                  "repo_dir": ".",
                  "max_files": 10
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let output = tools
        .call_tool("repo.files", &json!({"query": "lib"}))
        .unwrap();

    assert_eq!(output["files"], json!(["src/lib.rs"]));
    assert_eq!(output["artifacts"][0]["kind"], json!("repo_listing"));
    assert_eq!(tools.tool_capability("repo.files"), Some("code.read"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn repo_files_accepts_pattern_alias_for_query() {
    let dir = temp_dir("air-tools-repo-files-pattern-query");
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(dir.join("src/lib.rs"), "pub fn alpha() {}\n").unwrap();
    fs::write(dir.join("README.md"), "alpha docs\n").unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "repo.files": {
                  "kind": "repo_files",
                  "capability": "code.read",
                  "repo_dir": ".",
                  "max_files": 10
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let output = tools
        .call_tool("repo.files", &json!({"pattern": "lib"}))
        .unwrap();

    assert_eq!(output["files"], json!(["src/lib.rs"]));
    assert_eq!(output["query"], json!("lib"));
    assert_eq!(output["query_source"], json!("pattern"));
    assert_eq!(output["glob"], Value::Null);
    assert_eq!(output["glob_source"], json!("none"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn repo_files_accepts_pattern_alias_for_glob() {
    let dir = temp_dir("air-tools-repo-files-pattern-glob");
    fs::create_dir_all(dir.join("examples/code-agent")).unwrap();
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(dir.join("examples/code-agent/README.md"), "docs\n").unwrap();
    fs::write(dir.join("src/lib.rs"), "pub fn alpha() {}\n").unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "repo.files": {
                  "kind": "repo_files",
                  "capability": "code.read",
                  "repo_dir": ".",
                  "max_files": 10
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let output = tools
        .call_tool("repo.files", &json!({"pattern": "examples/code-agent/**"}))
        .unwrap();

    assert_eq!(output["files"], json!(["examples/code-agent/README.md"]));
    assert_eq!(output["query"], json!(""));
    assert_eq!(output["query_source"], json!("none"));
    assert_eq!(output["glob"], json!("examples/code-agent/**"));
    assert_eq!(output["glob_source"], json!("pattern"));
    assert_eq!(
        output["artifacts"][0]["metadata"]["glob_source"],
        json!("pattern")
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn repo_files_can_include_all_paths_for_open_ended_exploration() {
    let dir = temp_dir("air-tools-repo-files-include-all");
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(dir.join("src/lib.rs"), "pub fn alpha() {}\n").unwrap();
    fs::write(dir.join("README.md"), "alpha docs\n").unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "repo.files": {
                  "kind": "repo_files",
                  "capability": "code.read",
                  "repo_dir": ".",
                  "max_files": 10
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let output = tools
        .call_tool(
            "repo.files",
            &json!({"query": "does-not-match", "include_all": true}),
        )
        .unwrap();

    let files = output["files"].as_array().unwrap();
    assert!(files.iter().any(|file| file == "src/lib.rs"));
    assert!(files.iter().any(|file| file == "README.md"));
    assert_eq!(output["include_all"], json!(true));
    assert_eq!(
        output["artifacts"][0]["metadata"]["include_all"],
        json!(true)
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn repo_files_supports_smart_natural_language_queries() {
    let dir = temp_dir("air-tools-repo-files-smart");
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(dir.join("src/sum_math.js"), "function sum(values) {}\n").unwrap();
    fs::write(dir.join("src/other.js"), "function unrelated() {}\n").unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "repo.files": {
                  "kind": "repo_files",
                  "capability": "code.read",
                  "repo_dir": ".",
                  "max_files": 10
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let output = tools
        .call_tool(
            "repo.files",
            &json!({"query": "refactor the sum fixture implementation", "mode": "smart"}),
        )
        .unwrap();

    assert_eq!(output["mode"], json!("smart"));
    assert_eq!(output["effective_query"], json!("sum|fixture"));
    assert_eq!(output["files"], json!(["src/sum_math.js"]));
    assert_eq!(
        output["artifacts"][0]["metadata"]["effective_query"],
        json!("sum|fixture")
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn repo_files_include_all_smart_ranks_matching_paths_first_and_honors_max_files() {
    let dir = temp_dir("air-tools-repo-files-smart-include-all");
    fs::create_dir_all(dir.join("aaa")).unwrap();
    fs::create_dir_all(dir.join("zzz")).unwrap();
    fs::write(dir.join("aaa/unrelated.txt"), "nothing\n").unwrap();
    fs::write(dir.join("zzz/sum_fixture.js"), "function sum(values) {}\n").unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "repo.files": {
                  "kind": "repo_files",
                  "capability": "code.read",
                  "repo_dir": ".",
                  "max_files": 10
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let output = tools
        .call_tool(
            "repo.files",
            &json!({
                "query": "refactor the sum fixture implementation",
                "mode": "smart",
                "include_all": true,
                "max_files": 1
            }),
        )
        .unwrap();

    assert_eq!(output["files"], json!(["zzz/sum_fixture.js"]));
    assert_eq!(output["truncated"], json!(true));
    assert_eq!(output["artifacts"][0]["metadata"]["max_files"], json!(1));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn repo_files_rejects_unknown_mode() {
    let dir = temp_dir("air-tools-repo-files-mode");
    fs::write(dir.join("lib.rs"), "fn alpha() {}\n").unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "repo.files": {
                  "kind": "repo_files",
                  "capability": "code.read",
                  "repo_dir": "."
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let error = tools
        .call_tool("repo.files", &json!({"query": "alpha", "mode": "glob"}))
        .unwrap_err();

    assert!(error
        .to_string()
        .contains("input.mode must be fixed or smart"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn candidate_validate_rejects_missing_target_file() {
    let dir = temp_dir("air-tools-candidate-missing");
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "candidate.validate": {
                  "kind": "candidate_validate",
                  "capability": "code.read",
                  "base_dir": ".",
                  "allowed_test_commands": ["unit"]
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let error = tools
        .call_tool(
            "candidate.validate",
            &json!({
                "candidate": {
                    "target_path": "missing.js",
                    "related_files": [],
                    "test_command": "unit"
                }
            }),
        )
        .unwrap_err();

    assert!(error.to_string().contains("candidate.target_path"));
    assert!(error.to_string().contains("missing.js"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn candidate_validate_accepts_existing_target_and_allowlisted_test() {
    let dir = temp_dir("air-tools-candidate-valid");
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(dir.join("src/lib.js"), "module.exports = {}\n").unwrap();
    fs::write(dir.join("src/test.js"), "require('./lib')\n").unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "candidate.validate": {
                  "kind": "candidate_validate",
                  "capability": "code.read",
                  "base_dir": ".",
                  "allowed_test_commands": ["unit"]
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let output = tools
        .call_tool(
            "candidate.validate",
            &json!({
                "candidate": {
                    "target_path": "src/lib.js",
                    "related_files": ["src/test.js"],
                    "test_command": "unit"
                }
            }),
        )
        .unwrap();

    assert_eq!(output["valid"], json!(true));
    assert_eq!(output["target_path"], json!("src/lib.js"));
    assert_eq!(output["related_files"], json!(["src/test.js"]));
    assert_eq!(output["test_command"], json!("unit"));
    assert_eq!(
        tools.tool_capability("candidate.validate"),
        Some("code.read")
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn code_agent_self_tools_allow_project_verification_aliases() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let config_path = root.join("examples/code-agent/tools.self.json");
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let output = tools
        .call_tool(
            "candidate.validate",
            &json!({
                "candidate": {
                    "target_path": "crates/air-tools/src/lib.rs",
                    "related_files": ["crates/air-tools/src/tests.rs"],
                    "test_command": "verify_code_agent"
                }
            }),
        )
        .unwrap();

    assert_eq!(output["valid"], json!(true));
    assert_eq!(output["test_command"], json!("verify_code_agent"));

    let error = tools
        .call_tool(
            "test.run",
            &json!({
                "command": "cargo_test_package",
                "package": "not-a-package"
            }),
        )
        .unwrap_err();
    assert!(error.to_string().contains("not an allowed value"));
}

#[test]
fn repo_search_returns_structured_matches() {
    let dir = temp_dir("air-tools-repo-search");
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(
        dir.join("src/lib.rs"),
        "pub fn alpha() {}\npub fn beta() { alpha(); }\n",
    )
    .unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "repo.search": {
                  "kind": "repo_search",
                  "capability": "code.read",
                  "repo_dir": ".",
                  "max_matches": 5
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let output = tools
        .call_tool("repo.search", &json!({"query": "alpha", "path": "src"}))
        .unwrap();

    assert_eq!(output["matches"][0]["path"], json!("src/lib.rs"));
    assert_eq!(output["matches"][0]["line"], json!(1));
    assert!(output["artifacts"][0]["content"]
        .as_str()
        .unwrap()
        .contains("alpha"));
    assert_eq!(tools.tool_capability("repo.search"), Some("code.read"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn repo_search_treats_empty_path_as_unfiltered_repo_search() {
    let dir = temp_dir("air-tools-repo-search-empty-path");
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(dir.join("src/lib.rs"), "pub fn alpha() {}\n").unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "repo.search": {
                  "kind": "repo_search",
                  "capability": "code.read",
                  "repo_dir": ".",
                  "max_matches": 5
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let output = tools
        .call_tool("repo.search", &json!({"query": "alpha", "path": ""}))
        .unwrap();

    assert_eq!(output["matches"][0]["path"], json!("src/lib.rs"));
    assert_eq!(output["artifacts"][0]["metadata"]["paths"], json!([]));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn repo_search_supports_explicit_regex_mode() {
    let dir = temp_dir("air-tools-repo-search-regex");
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(
        dir.join("src/lib.rs"),
        "pub fn alpha() {}\nfn beta_value() {}\nfn gammaValue() {}\n",
    )
    .unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "repo.search": {
                  "kind": "repo_search",
                  "capability": "code.read",
                  "repo_dir": ".",
                  "max_matches": 5
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let output = tools
        .call_tool(
            "repo.search",
            &json!({"query": "fn [a-z]+_value", "mode": "regex", "path": "src"}),
        )
        .unwrap();

    assert_eq!(output["mode"], json!("regex"));
    assert_eq!(output["matches"].as_array().unwrap().len(), 1);
    assert!(output["matches"][0]["text"]
        .as_str()
        .unwrap()
        .contains("beta_value"));
    assert_eq!(output["artifacts"][0]["metadata"]["mode"], json!("regex"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn repo_search_accepts_common_llm_pattern_and_file_glob_aliases() {
    let dir = temp_dir("air-tools-repo-search-llm-aliases");
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::create_dir_all(dir.join("tests")).unwrap();
    fs::write(
        dir.join("src/math.js"),
        "function add(a, b) {\n  return a - b;\n}\n",
    )
    .unwrap();
    fs::write(dir.join("tests/math.test.js"), "assert(add(2, 3) === 5);\n").unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "repo.search": {
                  "kind": "repo_search",
                  "capability": "code.read",
                  "repo_dir": ".",
                  "max_matches": 5
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let output = tools
        .call_tool(
            "repo.search",
            &json!({"pattern": "add", "file_glob": "src/*.js"}),
        )
        .unwrap();

    assert_eq!(output["query"], json!("add"));
    assert_eq!(output["query_source"], json!("pattern"));
    assert_eq!(output["glob"], json!("src/*.js"));
    assert_eq!(output["matches"].as_array().unwrap().len(), 1);
    assert_eq!(output["matches"][0]["path"], json!("src/math.js"));
    assert_eq!(
        output["artifacts"][0]["metadata"]["query_source"],
        json!("pattern")
    );
    assert_eq!(
        output["artifacts"][0]["metadata"]["glob"],
        json!("src/*.js")
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn repo_search_supports_smart_natural_language_queries() {
    let dir = temp_dir("air-tools-repo-search-smart");
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(
        dir.join("src/math.js"),
        "function sum(values) {\n  return values.reduce((total, value) => total + value, 0);\n}\n",
    )
    .unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "repo.search": {
                  "kind": "repo_search",
                  "capability": "code.read",
                  "repo_dir": ".",
                  "max_matches": 5
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let output = tools
        .call_tool(
            "repo.search",
            &json!({"query": "refactor the sum fixture implementation to make it cleaner", "mode": "smart"}),
        )
        .unwrap();

    assert_eq!(output["mode"], json!("smart"));
    assert!(output["effective_query"].as_str().unwrap().contains("sum"));
    assert_eq!(output["matches"][0]["path"], json!("src/math.js"));
    assert!(output["artifacts"][0]["metadata"]["effective_query"]
        .as_str()
        .unwrap()
        .contains("sum"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn repo_search_falls_back_to_smart_for_multi_term_fixed_queries() {
    let dir = temp_dir("air-tools-repo-search-smart-fallback");
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(
        dir.join("src/lib.rs"),
        "fn path_segments() {}\nfn read_path() { path_segments(); }\n",
    )
    .unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "repo.search": {
                  "kind": "repo_search",
                  "capability": "code.read",
                  "repo_dir": ".",
                  "max_matches": 5
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let output = tools
        .call_tool("repo.search", &json!({"query": "path_segments read_path"}))
        .unwrap();

    assert_eq!(output["mode"], json!("smart"));
    assert_eq!(output["requested_mode"], json!("fixed"));
    assert!(output["effective_query"]
        .as_str()
        .unwrap()
        .contains("path_segments"));
    assert_eq!(output["matches"][0]["path"], json!("src/lib.rs"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn repo_search_supports_bounded_per_call_max_matches() {
    let dir = temp_dir("air-tools-repo-search-max-matches");
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(
        dir.join("src/lib.rs"),
        "alpha one\nalpha two\nalpha three\nalpha four\n",
    )
    .unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "repo.search": {
                  "kind": "repo_search",
                  "capability": "code.read",
                  "repo_dir": ".",
                  "max_matches": 3
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let output = tools
        .call_tool(
            "repo.search",
            &json!({"query": "alpha", "path": "src", "max_matches": 2}),
        )
        .unwrap();

    assert_eq!(output["matches"].as_array().unwrap().len(), 2);
    assert_eq!(output["truncated"], json!(true));
    assert_eq!(output["artifacts"][0]["metadata"]["max_matches"], json!(2));

    let capped = tools
        .call_tool(
            "repo.search",
            &json!({"query": "alpha", "path": "src", "max_matches": 99}),
        )
        .unwrap();
    assert_eq!(capped["matches"].as_array().unwrap().len(), 3);
    assert_eq!(capped["artifacts"][0]["metadata"]["max_matches"], json!(3));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn repo_search_rejects_unknown_mode() {
    let dir = temp_dir("air-tools-repo-search-mode");
    fs::write(dir.join("lib.rs"), "fn alpha() {}\n").unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "repo.search": {
                  "kind": "repo_search",
                  "capability": "code.read",
                  "repo_dir": "."
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let error = tools
        .call_tool("repo.search", &json!({"query": "alpha", "mode": "glob"}))
        .unwrap_err();

    assert!(error
        .to_string()
        .contains("input.mode must be fixed, regex, or smart"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn repo_context_returns_nearby_code_snippets() {
    let dir = temp_dir("air-tools-repo-context");
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(
        dir.join("src/lib.rs"),
        "line 1\nline 2\nfn alpha() {}\nline 4\nline 5\nline 6\nfn beta() { alpha(); }\nline 8\n",
    )
    .unwrap();
    fs::write(dir.join("src/other.rs"), "fn alpha_other() {}\n").unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "repo.context": {
                  "kind": "repo_context",
                  "capability": "code.read",
                  "repo_dir": ".",
                  "max_matches": 10,
                  "max_files": 1,
                  "context_lines": 1,
                  "max_bytes": 4096
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let output = tools
        .call_tool(
            "repo.context",
            &json!({"query": "alpha", "path": "src/lib.rs"}),
        )
        .unwrap();

    assert_eq!(output["snippets"].as_array().unwrap().len(), 2);
    assert_eq!(output["snippets"][0]["path"], json!("src/lib.rs"));
    assert_eq!(output["snippets"][0]["start_line"], json!(2));
    assert_eq!(output["snippets"][0]["end_line"], json!(4));
    assert!(output["artifacts"][0]["content"]
        .as_str()
        .unwrap()
        .contains("--- src/lib.rs:2-4 ---"));
    assert!(output["artifacts"][0]["content"]
        .as_str()
        .unwrap()
        .contains("3: fn alpha() {}"));
    assert_eq!(output["artifacts"][0]["kind"], json!("code_context"));
    assert_eq!(tools.tool_capability("repo.context"), Some("code.read"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn repo_context_supports_smart_natural_language_queries() {
    let dir = temp_dir("air-tools-repo-context-smart");
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(
        dir.join("src/math.js"),
        "line 1\nfunction sum(values) {\n  return values.reduce((total, value) => total + value, 0);\n}\nline 5\n",
    )
    .unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "repo.context": {
                  "kind": "repo_context",
                  "capability": "code.read",
                  "repo_dir": ".",
                  "max_matches": 5,
                  "max_files": 2
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let output = tools
        .call_tool(
            "repo.context",
            &json!({"query": "refactor the sum fixture implementation to make it cleaner", "mode": "smart"}),
        )
        .unwrap();

    assert_eq!(output["mode"], json!("smart"));
    assert_eq!(output["snippets"][0]["path"], json!("src/math.js"));
    assert!(output["snippets"][0]["content"]
        .as_str()
        .unwrap()
        .contains("function sum"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn repo_context_falls_back_to_smart_for_multi_term_fixed_queries() {
    let dir = temp_dir("air-tools-repo-context-smart-fallback");
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(
        dir.join("src/lib.rs"),
        "fn path_segments() {}\nfn read_path() { path_segments(); }\n",
    )
    .unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "repo.context": {
                  "kind": "repo_context",
                  "capability": "code.read",
                  "repo_dir": ".",
                  "max_matches": 5,
                  "max_files": 1,
                  "context_lines": 1
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let output = tools
        .call_tool("repo.context", &json!({"query": "path_segments read_path"}))
        .unwrap();

    assert_eq!(output["mode"], json!("smart"));
    assert_eq!(output["requested_mode"], json!("fixed"));
    assert_eq!(output["snippets"][0]["path"], json!("src/lib.rs"));
    assert!(output["snippets"][0]["content"]
        .as_str()
        .unwrap()
        .contains("read_path"));
    assert_eq!(
        output["artifacts"][0]["metadata"]["requested_mode"],
        json!("fixed")
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn repo_context_satisfies_read_before_file_ops_edit() {
    let dir = temp_dir("air-tools-repo-context-read-before-edit");
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(dir.join("src/lib.rs"), "pub fn alpha() {}\n").unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "repo.context": {
                  "kind": "repo_context",
                  "capability": "code.read",
                  "repo_dir": ".",
                  "max_matches": 5,
                  "max_files": 2
                },
                "file.ops": {
                  "kind": "file_ops",
                  "capability": "file.write",
                  "base_dir": ".",
                  "require_read": true
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    tools
        .call_tool(
            "repo.context",
            &json!({"query": "alpha", "path": "src/lib.rs"}),
        )
        .unwrap();
    let output = tools
        .call_tool(
            "file.ops",
            &json!({
                "operations": [{
                    "kind": "edit",
                    "path": "src/lib.rs",
                    "old_string": "alpha",
                    "new_string": "beta"
                }]
            }),
        )
        .unwrap();

    assert_eq!(output["applied"], json!(true));
    assert_eq!(
        fs::read_to_string(dir.join("src/lib.rs")).unwrap(),
        "pub fn beta() {}\n"
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn repo_context_supports_explicit_regex_mode() {
    let dir = temp_dir("air-tools-repo-context-regex");
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(
        dir.join("src/lib.rs"),
        "line 1\nfn alpha_value() {}\nline 3\nfn betaValue() {}\n",
    )
    .unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "repo.context": {
                  "kind": "repo_context",
                  "capability": "code.read",
                  "repo_dir": ".",
                  "max_matches": 10,
                  "max_files": 2,
                  "context_lines": 1,
                  "max_bytes": 4096
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let output = tools
        .call_tool(
            "repo.context",
            &json!({"query": "fn [a-z]+_value", "mode": "regex", "path": "src/lib.rs"}),
        )
        .unwrap();

    assert_eq!(output["mode"], json!("regex"));
    assert_eq!(output["matches"].as_array().unwrap().len(), 1);
    assert_eq!(output["snippets"][0]["path"], json!("src/lib.rs"));
    assert!(output["snippets"][0]["content"]
        .as_str()
        .unwrap()
        .contains("alpha_value"));
    assert_eq!(output["artifacts"][0]["metadata"]["mode"], json!("regex"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn repo_context_rejects_unknown_mode() {
    let dir = temp_dir("air-tools-repo-context-mode");
    fs::write(dir.join("lib.rs"), "fn alpha() {}\n").unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "repo.context": {
                  "kind": "repo_context",
                  "capability": "code.read",
                  "repo_dir": "."
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let error = tools
        .call_tool("repo.context", &json!({"query": "alpha", "mode": "glob"}))
        .unwrap_err();

    assert!(error
        .to_string()
        .contains("input.mode must be fixed, regex, or smart"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn repo_symbols_returns_lightweight_symbol_map() {
    let dir = temp_dir("air-tools-repo-symbols");
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(
        dir.join("src/lib.rs"),
        "pub struct Alpha {}\nfn beta_value() {}\nlet gamma = 1;\n",
    )
    .unwrap();
    fs::write(
        dir.join("src/app.ts"),
        "export function renderView() {}\nclass Panel {}\n",
    )
    .unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "repo.symbols": {
                  "kind": "repo_symbols",
                  "capability": "code.read",
                  "repo_dir": ".",
                  "max_symbols": 10,
                  "max_bytes": 4096
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let output = tools
        .call_tool("repo.symbols", &json!({"query": "alpha"}))
        .unwrap();

    assert_eq!(output["query"], json!("alpha"));
    assert_eq!(output["symbols"].as_array().unwrap().len(), 1);
    assert_eq!(output["symbols"][0]["kind"], json!("struct"));
    assert_eq!(output["symbols"][0]["name"], json!("Alpha"));
    assert_eq!(output["artifacts"][0]["kind"], json!("repo_symbols"));
    assert_eq!(tools.tool_capability("repo.symbols"), Some("code.read"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn repo_symbols_supports_path_and_glob_filters() {
    let dir = temp_dir("air-tools-repo-symbols-filters");
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(dir.join("src/lib.rs"), "pub struct Alpha {}\n").unwrap();
    fs::write(dir.join("src/app.ts"), "export function renderView() {}\n").unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "repo.symbols": {
                  "kind": "repo_symbols",
                  "capability": "code.read",
                  "repo_dir": ".",
                  "max_symbols": 10
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let output = tools
        .call_tool(
            "repo.symbols",
            &json!({"path": "src", "glob": "*.ts", "max_symbols": 2}),
        )
        .unwrap();

    assert_eq!(output["symbols"].as_array().unwrap().len(), 1);
    assert_eq!(output["symbols"][0]["path"], json!("src/app.ts"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn repo_symbols_supports_smart_natural_language_queries() {
    let dir = temp_dir("air-tools-repo-symbols-smart");
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(
        dir.join("src/math.js"),
        "function sum(values) {\n  return values.reduce((total, value) => total + value, 0);\n}\nfunction unrelated() {}\n",
    )
    .unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "repo.symbols": {
                  "kind": "repo_symbols",
                  "capability": "code.read",
                  "repo_dir": ".",
                  "max_symbols": 10
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let output = tools
        .call_tool(
            "repo.symbols",
            &json!({"query": "refactor the sum fixture implementation to make it cleaner", "mode": "smart"}),
        )
        .unwrap();

    assert_eq!(output["mode"], json!("smart"));
    assert!(output["effective_query"].as_str().unwrap().contains("sum"));
    assert_eq!(output["symbols"].as_array().unwrap().len(), 1);
    assert_eq!(output["symbols"][0]["name"], json!("sum"));
    assert_eq!(output["artifacts"][0]["metadata"]["mode"], json!("smart"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn repo_symbols_rejects_unknown_mode() {
    let dir = temp_dir("air-tools-repo-symbols-mode");
    fs::write(dir.join("lib.rs"), "fn alpha() {}\n").unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "repo.symbols": {
                  "kind": "repo_symbols",
                  "capability": "code.read",
                  "repo_dir": "."
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let error = tools
        .call_tool("repo.symbols", &json!({"query": "alpha", "mode": "glob"}))
        .unwrap_err();

    assert!(error
        .to_string()
        .contains("input.mode must be fixed or smart"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn repo_references_returns_definition_references_and_snippets() {
    let dir = temp_dir("air-tools-repo-references");
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(
        dir.join("src/lib.rs"),
        "pub struct Alpha {}\nimpl Alpha { fn new() -> Alpha { Alpha {} } }\nlet Alphabet = 1;\n",
    )
    .unwrap();
    fs::write(
        dir.join("src/app.ts"),
        "function useAlpha(value: Alpha) { return value }\n",
    )
    .unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "repo.references": {
                  "kind": "repo_references",
                  "capability": "code.read",
                  "repo_dir": ".",
                  "max_matches": 10,
                  "max_files": 4,
                  "context_lines": 1,
                  "max_bytes": 4096
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let output = tools
        .call_tool("repo.references", &json!({"symbol": "Alpha"}))
        .unwrap();

    assert_eq!(output["symbol"], json!("Alpha"));
    assert_eq!(output["definitions"].as_array().unwrap().len(), 1);
    assert_eq!(output["definitions"][0]["kind"], json!("struct"));
    assert_eq!(output["references"].as_array().unwrap().len(), 3);
    assert!(output["references"]
        .as_array()
        .unwrap()
        .iter()
        .all(|reference| reference["text"] != json!("let Alphabet = 1;")));
    assert!(!output["snippets"].as_array().unwrap().is_empty());
    assert_eq!(output["artifacts"][0]["kind"], json!("repo_references"));
    assert_eq!(tools.tool_capability("repo.references"), Some("code.read"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn repo_references_rejects_non_identifier_symbols() {
    let dir = temp_dir("air-tools-repo-references-invalid");
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "repo.references": {
                  "kind": "repo_references",
                  "repo_dir": "."
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let error = tools
        .call_tool("repo.references", &json!({"symbol": "Alpha Beta"}))
        .unwrap_err();

    assert!(error
        .to_string()
        .contains("input.symbol must be an identifier-like token"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn diagnostic_context_returns_source_snippets_for_command_diagnostics() {
    let dir = temp_dir("air-tools-diagnostic-context");
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(
        dir.join("src/lib.rs"),
        "line 1\nline 2\nfn broken() {}\nline 4\nline 5\n",
    )
    .unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "diagnostic.context": {
                  "kind": "diagnostic_context",
                  "capability": "code.read",
                  "repo_dir": ".",
                  "max_diagnostics": 5,
                  "context_lines": 1,
                  "max_bytes": 4096
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let output = tools
        .call_tool(
            "diagnostic.context",
            &json!({
                "diagnostics": [{
                    "source": "command_run",
                    "severity": "error",
                    "path": "src/lib.rs",
                    "line": 3,
                    "column": 4,
                    "message": "expected value"
                }]
            }),
        )
        .unwrap();

    assert_eq!(output["snippets"].as_array().unwrap().len(), 1);
    assert_eq!(output["snippets"][0]["path"], json!("src/lib.rs"));
    assert_eq!(output["snippets"][0]["start_line"], json!(2));
    assert_eq!(output["snippets"][0]["end_line"], json!(4));
    assert!(output["snippets"][0]["content"]
        .as_str()
        .unwrap()
        .contains("3: fn broken() {}"));
    assert_eq!(output["artifacts"][0]["kind"], json!("diagnostic_context"));
    assert_eq!(
        output["artifacts"][0]["metadata"]["provider"],
        json!("diagnostic_context")
    );
    assert!(output["unreadable"].as_array().unwrap().is_empty());
    assert_eq!(
        tools.tool_capability("diagnostic.context"),
        Some("code.read")
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn diagnostic_context_returns_path_snippet_without_line() {
    let dir = temp_dir("air-tools-diagnostic-context-path-only");
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(dir.join("src/lib.rs"), "first\nsecond\nthird\n").unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "diagnostic.context": {
                  "kind": "diagnostic_context",
                  "capability": "code.read",
                  "repo_dir": ".",
                  "context_lines": 2,
                  "max_bytes": 4096
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let output = tools
        .call_tool(
            "diagnostic.context",
            &json!({
                "diagnostics": [{
                    "source": "file.ops",
                    "severity": "error",
                    "path": "src/lib.rs",
                    "message": "old_string was not found"
                }]
            }),
        )
        .unwrap();

    assert_eq!(output["snippets"].as_array().unwrap().len(), 1);
    assert_eq!(output["snippets"][0]["path"], json!("src/lib.rs"));
    assert_eq!(output["snippets"][0]["start_line"], json!(1));
    assert_eq!(output["snippets"][0]["end_line"], json!(3));
    assert_eq!(
        output["snippets"][0]["path_only_diagnostic_indexes"],
        json!([0])
    );
    assert!(output["snippets"][0]["content"]
        .as_str()
        .unwrap()
        .contains("1: first"));
    assert!(output["unreadable"].as_array().unwrap().is_empty());
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn diagnostic_context_accepts_missing_diagnostics_as_empty_observation() {
    let dir = temp_dir("air-tools-diagnostic-context-empty");
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "diagnostic.context": {
                  "kind": "diagnostic_context",
                  "capability": "code.read",
                  "repo_dir": "."
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let output = tools.call_tool("diagnostic.context", &json!({})).unwrap();

    assert_eq!(output["diagnostic_count"], json!(0));
    assert!(output["snippets"].as_array().unwrap().is_empty());
    assert!(output["unreadable"].as_array().unwrap().is_empty());
    assert_eq!(
        output["artifacts"][0]["metadata"]["diagnostic_count"],
        json!(0)
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn diagnostic_context_satisfies_read_before_file_ops_edit() {
    let dir = temp_dir("air-tools-diagnostic-context-read-before-edit");
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(dir.join("src/lib.rs"), "pub fn broken() {}\n").unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "diagnostic.context": {
                  "kind": "diagnostic_context",
                  "capability": "code.read",
                  "repo_dir": "."
                },
                "file.ops": {
                  "kind": "file_ops",
                  "capability": "file.write",
                  "base_dir": ".",
                  "require_read": true
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    tools
        .call_tool(
            "diagnostic.context",
            &json!({
                "diagnostics": [{
                    "source": "command_run",
                    "severity": "error",
                    "path": "src/lib.rs",
                    "line": 1,
                    "column": 8,
                    "message": "broken function"
                }]
            }),
        )
        .unwrap();
    let output = tools
        .call_tool(
            "file.ops",
            &json!({
                "operations": [{
                    "kind": "edit",
                    "path": "src/lib.rs",
                    "old_string": "broken",
                    "new_string": "fixed"
                }]
            }),
        )
        .unwrap();

    assert_eq!(output["applied"], json!(true));
    assert_eq!(
        fs::read_to_string(dir.join("src/lib.rs")).unwrap(),
        "pub fn fixed() {}\n"
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn diagnostic_context_read_snapshot_rejects_stale_file_ops_edit() {
    let dir = temp_dir("air-tools-diagnostic-context-stale-edit");
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(dir.join("src/lib.rs"), "pub fn broken() {}\n").unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "diagnostic.context": {
                  "kind": "diagnostic_context",
                  "capability": "code.read",
                  "repo_dir": "."
                },
                "file.ops": {
                  "kind": "file_ops",
                  "capability": "file.write",
                  "base_dir": ".",
                  "require_read": true
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();
    tools
        .call_tool(
            "diagnostic.context",
            &json!({
                "diagnostics": [{
                    "source": "command_run",
                    "severity": "error",
                    "path": "src/lib.rs",
                    "line": 1,
                    "column": 8,
                    "message": "broken function"
                }]
            }),
        )
        .unwrap();
    std::thread::sleep(Duration::from_millis(20));
    fs::write(dir.join("src/lib.rs"), "pub fn externally_changed() {}\n").unwrap();

    let error = tools
        .call_tool(
            "file.ops",
            &json!({
                "operations": [{
                    "kind": "edit",
                    "path": "src/lib.rs",
                    "old_string": "broken",
                    "new_string": "fixed"
                }]
            }),
        )
        .unwrap_err();

    assert!(error
        .to_string()
        .contains("modified after it was last read"));
    assert_eq!(
        fs::read_to_string(dir.join("src/lib.rs")).unwrap(),
        "pub fn externally_changed() {}\n"
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn diagnostic_context_skips_paths_outside_repo() {
    let dir = temp_dir("air-tools-diagnostic-context-boundary");
    fs::write(dir.join("lib.rs"), "fn alpha() {}\n").unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "diagnostic.context": {
                  "kind": "diagnostic_context",
                  "capability": "code.read",
                  "repo_dir": "."
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let output = tools
        .call_tool(
            "diagnostic.context",
            &json!({
                "diagnostics": [{
                    "path": "../secret.txt",
                    "line": 1
                }]
            }),
        )
        .unwrap();

    assert!(output["snippets"].as_array().unwrap().is_empty());
    assert_eq!(output["unreadable"].as_array().unwrap().len(), 1);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn command_run_executes_allowlisted_command() {
    let dir = temp_dir("air-tools-command-run");
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "test.run": {
                  "kind": "command_run",
                  "capability": "code.test",
                  "cwd": ".",
                  "commands": {
                    "cargo_version": ["cargo", "--version"]
                  },
                  "timeout_seconds": 10
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let output = tools
        .call_tool("test.run", &json!({"command": "cargo_version"}))
        .unwrap();

    assert_eq!(output["success"], json!(true));
    assert!(output["log"].as_str().unwrap().contains("cargo"));
    assert_eq!(output["artifacts"][0]["kind"], json!("test_log"));
    assert_eq!(tools.tool_capability("test.run"), Some("code.test"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn command_run_accepts_test_command_alias() {
    let dir = temp_dir("air-tools-command-run-test-command-alias");
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "test.run": {
                  "kind": "command_run",
                  "capability": "code.test",
                  "cwd": ".",
                  "commands": {
                    "cargo_version": ["cargo", "--version"]
                  },
                  "timeout_seconds": 10
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let output = tools
        .call_tool("test.run", &json!({"test_command": "cargo_version"}))
        .unwrap();

    assert_eq!(output["success"], json!(true));
    assert!(output["log"].as_str().unwrap().contains("cargo"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn command_run_saves_full_log_when_truncated() {
    let dir = temp_dir("air-tools-command-run-truncated-log");
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "test.run": {
                  "kind": "command_run",
                  "capability": "code.test",
                  "cwd": ".",
                  "commands": {
                    "long_log": ["sh", "-c", "printf 'line-one\\nline-two\\nline-three\\n'"]
                  },
                  "timeout_seconds": 10,
                  "max_bytes": 12
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let output = tools
        .call_tool("test.run", &json!({"command": "long_log"}))
        .unwrap();

    assert_eq!(output["success"], json!(true));
    assert_eq!(output["truncated"], json!(true));
    assert_eq!(output["log"], json!("line-one\nlin"));
    let full_log_path = output["full_log_path"].as_str().unwrap();
    assert!(full_log_path.ends_with(".log"), "{full_log_path}");
    assert_eq!(
        fs::read_to_string(full_log_path).unwrap(),
        "line-one\nline-two\nline-three\n"
    );
    assert!(output["truncation_hint"]
        .as_str()
        .unwrap()
        .contains("Full command output saved to"));
    assert_eq!(
        output["artifacts"][0]["metadata"]["full_log_path"],
        json!(full_log_path)
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn command_run_can_tail_truncated_log_preview() {
    let dir = temp_dir("air-tools-command-run-tail-log");
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "test.run": {
                  "kind": "command_run",
                  "capability": "code.test",
                  "cwd": ".",
                  "commands": {
                    "tail_log": ["sh", "-c", "printf 'setup noise\\nERROR tail\\n'"]
                  },
                  "timeout_seconds": 10,
                  "max_bytes": 11,
                  "truncation_direction": "tail"
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let output = tools
        .call_tool("test.run", &json!({"command": "tail_log"}))
        .unwrap();

    assert_eq!(output["truncated"], json!(true));
    assert_eq!(output["log"], json!("ERROR tail\n"));
    assert_eq!(
        fs::read_to_string(output["full_log_path"].as_str().unwrap()).unwrap(),
        "setup noise\nERROR tail\n"
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn command_run_full_log_path_can_be_searched_by_file_search() {
    let dir = temp_dir("air-tools-command-run-search-full-log");
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "test.run": {
                  "kind": "command_run",
                  "capability": "code.test",
                  "cwd": ".",
                  "commands": {
                    "long_log": ["sh", "-c", "printf 'setup noise\\nERROR searchable tail\\n'"]
                  },
                  "timeout_seconds": 10,
                  "max_bytes": 8,
                  "truncation_direction": "tail"
                },
                "file.search": {
                  "kind": "file_search",
                  "capability": "file.read",
                  "base_dir": ".",
                  "max_matches": 8,
                  "max_context_lines": 2,
                  "max_line_chars": 200,
                  "max_bytes": 4096
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let command = tools
        .call_tool("test.run", &json!({"command": "long_log"}))
        .unwrap();
    assert_eq!(command["truncated"], json!(true));
    let full_log_path = command["full_log_path"].as_str().unwrap();

    let search = tools
        .call_tool(
            "file.search",
            &json!({
                "path": full_log_path,
                "pattern": "ERROR searchable tail",
                "context_lines": 1
            }),
        )
        .unwrap();

    assert_eq!(search["match_count"], json!(1));
    assert_eq!(search["matches"][0]["line"], json!("ERROR searchable tail"));
    let expected_log_dir = dir.canonicalize().unwrap().join(".air").join("tool-output");
    assert!(
        Path::new(full_log_path).starts_with(&expected_log_dir),
        "full_log_path should be under .air/tool-output, got: {full_log_path}"
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn command_run_allows_empty_command_map_until_called() {
    let dir = temp_dir("air-tools-command-run-empty-command-map");
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "test.run": {
                  "kind": "command_run",
                  "capability": "code.test",
                  "cwd": ".",
                  "commands": {},
                  "timeout_seconds": 10
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let error = tools
        .call_tool("test.run", &json!({"command": "unit"}))
        .unwrap_err();

    assert!(error
        .to_string()
        .contains("tool test.run command unit is not configured"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn command_run_extracts_structured_diagnostics() {
    let dir = temp_dir("air-tools-command-run-diagnostics");
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "test.run": {
                  "kind": "command_run",
                  "capability": "code.test",
                  "cwd": ".",
                  "commands": {
                    "tsc": [
                      "node",
                      "-e",
                      "console.error('src/main.ts(4,9): error TS2304: Cannot find name x.'); process.exit(2)"
                    ]
                  },
                  "timeout_seconds": 10
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let output = tools
        .call_tool("test.run", &json!({"command": "tsc"}))
        .unwrap();

    assert_eq!(output["success"], json!(false));
    assert_eq!(output["diagnostics"][0]["path"], json!("src/main.ts"));
    assert_eq!(output["diagnostics"][0]["line"], json!(4));
    assert_eq!(output["diagnostics"][0]["column"], json!(9));
    assert_eq!(output["diagnostics"][0]["severity"], json!("error"));
    assert!(output["diagnostics"][0]["message"]
        .as_str()
        .unwrap()
        .contains("TS2304"));
    assert_eq!(
        output["artifacts"][0]["metadata"]["diagnostics_count"],
        json!(1)
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn command_run_renders_constrained_template_parameters() {
    let dir = temp_dir("air-tools-command-run-parameters");
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "test.run": {
                  "kind": "command_run",
                  "capability": "code.test",
                  "cwd": ".",
                  "commands": {
                    "echo_test": ["node", "-e", "console.log(process.argv[1])", "{{ test_filter }}"]
                  },
                  "parameters": {
                    "test_filter": {
                      "allow": "identifier",
                      "max_chars": 80
                    }
                  },
                  "timeout_seconds": 10
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let output = tools
        .call_tool(
            "test.run",
            &json!({"command": "echo_test", "test_filter": "module::test_name"}),
        )
        .unwrap();

    assert_eq!(output["success"], json!(true));
    assert_eq!(output["argv"][3], json!("module::test_name"));
    assert!(output["log"]
        .as_str()
        .unwrap()
        .contains("module::test_name"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn command_run_accepts_nested_args_template_parameters() {
    let dir = temp_dir("air-tools-command-run-nested-args");
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "test.run": {
                  "kind": "command_run",
                  "capability": "code.test",
                  "cwd": ".",
                  "commands": {
                    "echo_test": ["node", "-e", "console.log(process.argv[1])", "{{ test_filter }}"]
                  },
                  "parameters": {
                    "test_filter": {
                      "allow": "identifier",
                      "max_chars": 80
                    }
                  },
                  "timeout_seconds": 10
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let output = tools
        .call_tool(
            "test.run",
            &json!({
                "command": "echo_test",
                "args": {"test_filter": "module::test_name"}
            }),
        )
        .unwrap();

    assert_eq!(output["success"], json!(true));
    assert_eq!(output["argv"][3], json!("module::test_name"));
    assert!(output["log"]
        .as_str()
        .unwrap()
        .contains("module::test_name"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn command_run_rejects_undeclared_or_invalid_template_parameters() {
    let dir = temp_dir("air-tools-command-run-parameter-policy");
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "test.run": {
                  "kind": "command_run",
                  "cwd": ".",
                  "commands": {
                    "echo_test": ["node", "-e", "console.log(process.argv[1])", "{{test_filter}}"]
                  },
                  "parameters": {
                    "test_filter": {
                      "allow": "identifier",
                      "values": ["safe_test"]
                    }
                  }
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let error = tools
        .call_tool(
            "test.run",
            &json!({"command": "echo_test", "test_filter": "--eval=bad"}),
        )
        .unwrap_err();

    assert!(error.to_string().contains("not an allowed value"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn extract_command_diagnostics_parses_rustc_location_blocks() {
    let diagnostics = extract_command_diagnostics(
        "error[E0425]: cannot find value `missing` in this scope\n  --> src/lib.rs:12:5\n",
        10,
    );

    assert_eq!(diagnostics[0]["severity"], json!("error"));
    assert_eq!(diagnostics[0]["path"], json!("src/lib.rs"));
    assert_eq!(diagnostics[0]["line"], json!(12));
    assert_eq!(diagnostics[0]["column"], json!(5));
    assert!(diagnostics[0]["message"]
        .as_str()
        .unwrap()
        .contains("cannot find value"));
}

#[test]
fn extract_command_diagnostics_parses_python_tracebacks() {
    let diagnostics = extract_command_diagnostics(
        r#"Traceback (most recent call last):
  File "/tmp/project/run.py", line 8, in <module>
    main()
  File "src/app.py", line 3, in main
    assert False
AssertionError: broken invariant
"#,
        10,
    );

    assert_eq!(diagnostics[0]["severity"], json!("error"));
    assert_eq!(diagnostics[0]["path"], json!("src/app.py"));
    assert_eq!(diagnostics[0]["line"], json!(3));
    assert_eq!(diagnostics[0]["column"], json!(1));
    assert_eq!(
        diagnostics[0]["message"],
        json!("AssertionError: broken invariant")
    );
}

#[test]
fn extract_command_diagnostics_parses_line_only_colon_diagnostics() {
    let diagnostics = extract_command_diagnostics(
        "src/app.py:12: error: Incompatible return value type\n\
             tests/test_app.py:7: AssertionError: expected true\n\
             src/app.py:12: in handler\n",
        10,
    );

    assert_eq!(diagnostics.len(), 2);
    assert_eq!(diagnostics[0]["severity"], json!("error"));
    assert_eq!(diagnostics[0]["path"], json!("src/app.py"));
    assert_eq!(diagnostics[0]["line"], json!(12));
    assert_eq!(diagnostics[0]["column"], json!(1));
    assert_eq!(
        diagnostics[0]["message"],
        json!("Incompatible return value type")
    );
    assert_eq!(diagnostics[1]["severity"], json!("error"));
    assert_eq!(diagnostics[1]["path"], json!("tests/test_app.py"));
    assert_eq!(diagnostics[1]["line"], json!(7));
    assert_eq!(
        diagnostics[1]["message"],
        json!("AssertionError: expected true")
    );
}

#[test]
fn extract_command_diagnostics_parses_file_context_lint_blocks() {
    let diagnostics = extract_command_diagnostics(
        r#"src/main.ts
  12:5  error  Unexpected any.  @typescript-eslint/no-explicit-any
  18:1  warning  Missing return type  @typescript-eslint/explicit-function-return-type
✖ 2 problems
"#,
        10,
    );

    assert_eq!(diagnostics.len(), 2);
    assert_eq!(diagnostics[0]["severity"], json!("error"));
    assert_eq!(diagnostics[0]["path"], json!("src/main.ts"));
    assert_eq!(diagnostics[0]["line"], json!(12));
    assert_eq!(diagnostics[0]["column"], json!(5));
    assert_eq!(
        diagnostics[0]["message"],
        json!("Unexpected any.  @typescript-eslint/no-explicit-any")
    );
    assert_eq!(diagnostics[1]["severity"], json!("warning"));
    assert_eq!(diagnostics[1]["line"], json!(18));
    assert_eq!(diagnostics[1]["column"], json!(1));
}

#[test]
fn local_docs_search_returns_artifacts_for_documents() {
    let output = search_docs(
        &json!({"query": "provenance"}),
        &[LocalDoc {
            id: "doc-1".to_string(),
            title: "Provenance".to_string(),
            content: "AIR provenance artifacts".to_string(),
        }],
        3,
    );

    assert_eq!(output["documents"][0]["id"], json!("doc-1"));
    assert_eq!(output["artifacts"][0]["id"], json!("doc-1"));
    assert_eq!(output["artifacts"][0]["kind"], json!("doc_chunk"));
    assert_eq!(output["artifacts"][0]["uri"], json!("local-doc://doc-1"));
}

#[test]
fn playwright_search_invokes_script_and_returns_artifacts() {
    let dir = temp_dir("air-tools-playwright-search");
    fs::write(
            dir.join("search.cjs"),
            r#"
const chunks = [];
process.stdin.on('data', chunk => chunks.push(chunk));
process.stdin.on('end', () => {
  const input = JSON.parse(Buffer.concat(chunks).toString('utf8'));
  process.stdout.write(JSON.stringify({
    query: input.query,
    received: input,
    documents: [{ id: 'web:example', title: 'Example', url: 'https://example.com', content: 'Example content' }],
    artifacts: [{ id: 'web:example', kind: 'web_page', title: 'Example', uri: 'https://example.com', content: 'Example content', metadata: { provider: 'playwright_search' } }]
  }));
});
"#,
        )
        .unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "web.search": {
                  "kind": "playwright_search",
                  "capability": "network.search",
                  "script_path": "search.cjs",
                  "query_variants": ["{{query}} GitHub", "{{query}} docs"],
                  "max_results": 2,
                  "max_results_per_query": 4,
                  "max_per_domain": 1,
                  "max_content_chars": 1000,
                  "include_domains": ["github.com", "openclaw.ai"],
                  "exclude_domains": ["example-spam.test"],
                  "required_terms": ["openclaw"],
                  "exclude_terms": ["spam"],
                  "page_concurrency": 2,
                  "navigation_timeout_ms": 1000,
                  "overall_timeout_ms": 4000,
                  "search_delay_ms": 100,
                  "retry_count": 1,
                  "user_agent": "AIR test",
                  "fetch_pages": false,
                  "cache_dir": "cache",
                  "cache_ttl_seconds": 3600,
                  "timeout_seconds": 5
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let output = tools
        .call_tool("web.search", &json!({"query": "openclaw"}))
        .unwrap();

    assert_eq!(output["query"], json!("openclaw"));
    assert_eq!(
        output["received"]["query_variants"],
        json!(["openclaw GitHub", "openclaw docs"])
    );
    assert_eq!(output["received"]["max_results_per_query"], json!(4));
    assert_eq!(output["received"]["max_per_domain"], json!(1));
    assert_eq!(
        output["received"]["include_domains"],
        json!(["github.com", "openclaw.ai"])
    );
    assert_eq!(
        output["received"]["exclude_domains"],
        json!(["example-spam.test"])
    );
    assert_eq!(output["received"]["required_terms"], json!(["openclaw"]));
    assert_eq!(output["received"]["exclude_terms"], json!(["spam"]));
    assert_eq!(output["received"]["page_concurrency"], json!(2));
    assert_eq!(output["received"]["overall_timeout_ms"], json!(4000));
    assert_eq!(output["received"]["search_delay_ms"], json!(100));
    assert_eq!(output["received"]["retry_count"], json!(1));
    assert_eq!(output["received"]["user_agent"], json!("AIR test"));
    assert_eq!(output["received"]["fetch_pages"], json!(false));
    assert!(output["received"]["cache_dir"]
        .as_str()
        .unwrap()
        .ends_with("cache"));
    assert_eq!(output["received"]["cache_ttl_seconds"], json!(3600));
    assert_eq!(output["artifacts"][0]["id"], json!("web:example"));
    assert_eq!(tools.tool_capability("web.search"), Some("network.search"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn playwright_page_audit_invokes_script_and_bounds_paths() {
    let dir = temp_dir("air-tools-playwright-page-audit");
    fs::write(
        dir.join("index.html"),
        "<!doctype html><html><body>ok</body></html>",
    )
    .unwrap();
    fs::write(
            dir.join("audit.cjs"),
            r#"
const chunks = [];
process.stdin.on('data', chunk => chunks.push(chunk));
process.stdin.on('end', () => {
  const input = JSON.parse(Buffer.concat(chunks).toString('utf8'));
  process.stdout.write(JSON.stringify({
    target: input.path || input.url,
    received: input,
    success: true,
    viewport_count: input.viewports.length,
    viewports: [{ width: input.viewports[0].width, height: input.viewports[0].height, horizontal_overflow: false, overlap_count: 0, screenshot_path: input.screenshot_dir + '/page.png' }],
    diagnostics: [],
    artifacts: [{ id: 'browser-screenshot:test', kind: 'browser_screenshot', title: 'audit', uri: input.screenshot_dir + '/page.png', content: '', metadata: { provider: 'playwright_page_audit' } }]
  }));
});
"#,
        )
        .unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "browser.audit": {
                  "kind": "playwright_page_audit",
                  "capability": "browser.audit",
                  "script_path": "audit.cjs",
                  "base_dir": ".",
                  "screenshot_dir": "screens",
                  "viewports": [{ "label": "desktop", "width": 1280, "height": 900 }],
                  "required_text": ["{{term}}"],
                  "forbidden_text": ["markdown fence"],
                  "require_canvas": true,
                  "navigation_timeout_ms": 1000,
                  "max_text_chars": 500,
                  "timeout_seconds": 5
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let output = tools
        .call_tool(
            "browser.audit",
            &json!({"path": "index.html", "term": "ok"}),
        )
        .unwrap();

    assert_eq!(output["success"], json!(true));
    assert_eq!(output["viewport_count"], json!(1));
    assert!(output["received"]["path"]
        .as_str()
        .unwrap()
        .ends_with("index.html"));
    assert_eq!(output["received"]["viewports"][0]["width"], json!(1280));
    assert_eq!(output["received"]["required_text"], json!(["ok"]));
    assert_eq!(
        output["received"]["forbidden_text"],
        json!(["markdown fence"])
    );
    assert_eq!(output["received"]["require_canvas"], json!(true));
    assert!(output["received"]["screenshot_dir"]
        .as_str()
        .unwrap()
        .ends_with("screens"));
    assert_eq!(output["artifacts"][0]["kind"], json!("browser_screenshot"));
    assert_eq!(
        tools.tool_capability("browser.audit"),
        Some("browser.audit")
    );

    let error = tools
        .call_tool("browser.audit", &json!({"path": "../outside.html"}))
        .unwrap_err();
    assert!(error.to_string().contains("input.path"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn playwright_page_audit_rejects_empty_text_assertions() {
    let dir = temp_dir("air-tools-playwright-page-audit-invalid-text");
    fs::write(dir.join("audit.cjs"), "").unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "browser.audit": {
                  "kind": "playwright_page_audit",
                  "script_path": "audit.cjs",
                  "base_dir": ".",
                  "required_text": ["ok", ""]
                }
              }
            }"#,
    );

    let error = ConfigTools::from_file(config_path).unwrap_err();

    assert!(error.to_string().contains("required_text"));
    assert!(error.to_string().contains("must not be empty"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn web_fetch_config_validates_limits() {
    let dir = temp_dir("air-tools-web-fetch-config");
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "web.fetch": {
                  "kind": "web_fetch",
                  "capability": "network.fetch",
                  "timeout_seconds": 0
                }
              }
            }"#,
    );

    let error = ConfigTools::from_file(config_path).unwrap_err();

    assert!(error.to_string().contains("timeout_seconds"));
    assert!(error.to_string().contains("greater than 0"));
    let _ = fs::remove_dir_all(dir);
}
