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
fn tool_config_workspace_dir_rebases_file_tool_roots() {
    let workspace = temp_dir("air-tools-workspace-dir");
    fs::write(workspace.join("note.txt"), "hello from workspace").unwrap();
    let config_dir = workspace.join("configs");
    fs::create_dir_all(&config_dir).unwrap();
    let config_path = write_config(
        &config_dir,
        &json!({
            "workspace_dir": workspace,
            "tools": {
                "read": {
                    "kind": "file_read",
                    "capability": "file.read",
                    "base_dir": "."
                }
            }
        })
        .to_string(),
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let output = tools
        .call_tool("read", &json!({ "path": "note.txt" }))
        .unwrap();

    assert_eq!(output["content"], json!("00001| hello from workspace"));
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

    assert_eq!(output["content"], json!("00001| hello fr"));
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
    assert_eq!(
        output["files"][0]["content"],
        json!("00001| alpha\n00002| beta")
    );
    assert_eq!(output["max_bytes_per_file"], json!(1024));
    assert_eq!(output["files"][1]["match_line"], json!(2));
    assert_eq!(
        output["files"][1]["content"],
        json!("00001| first\n00002| needle\n00003| last")
    );
    assert_eq!(output["files"][1]["content_format"], json!("line_numbered"));
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
    assert_eq!(output["files"][0]["content"], json!("00001| abcde"));
    assert_eq!(output["files"][0]["truncated"], json!(true));

    let capped = tools
        .call_tool(
            "file.read_many",
            &json!({"files": ["one.txt"], "max_bytes_per_file": 99}),
        )
        .unwrap();
    assert_eq!(capped["max_bytes_per_file"], json!(12));
    assert_eq!(capped["files"][0]["content"], json!("00001| abcdefghijkl"));
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
    assert_eq!(output["content"], json!("00001| abcdefgh"));
    assert_eq!(output["max_bytes"], json!(8));
    assert_eq!(output["truncated"], json!(true));

    let capped = tools
        .call_tool("file.read", &json!({"path": "note.txt", "max_bytes": 99}))
        .unwrap();
    assert_eq!(capped["content"], json!("00001| abcdefghijklmnop"));
    assert_eq!(capped["max_bytes"], json!(16));
    assert_eq!(capped["truncated"], json!(true));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn file_read_accepts_opencode_file_path_offset_limit() {
    let dir = temp_dir("air-tools-file-read-opencode");
    fs::write(dir.join("note.txt"), "zero\none\ntwo\nthree\n").unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "read": {
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
            "read",
            &json!({"filePath": "note.txt", "offset": 1, "limit": 2}),
        )
        .unwrap();

    assert!(output["path"]
        .as_str()
        .is_some_and(|path| path.ends_with("note.txt")));
    assert_eq!(output["start_line"], json!(2));
    assert_eq!(output["end_line"], json!(3));
    assert_eq!(output["content"], json!("00002| one\n00003| two"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn file_read_offset_without_limit_uses_bounded_window() {
    let dir = temp_dir("air-tools-file-read-offset-default-window");
    let content = (1..=350)
        .map(|line| format!("line {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(dir.join("note.txt"), content).unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "read": {
                  "kind": "file_read",
                  "capability": "file.read",
                  "base_dir": "."
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let output = tools
        .call_tool("read", &json!({"filePath": "note.txt", "offset": 100}))
        .unwrap();

    assert_eq!(output["start_line"], json!(101));
    assert_eq!(output["end_line"], json!(350));
    let content = output["content"].as_str().unwrap();
    assert!(content.contains("00101| line 101"));
    assert!(content.contains("00350| line 350"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn file_read_start_line_without_limit_uses_bounded_window() {
    let dir = temp_dir("air-tools-file-read-start-line-default-window");
    let content = (1..=350)
        .map(|line| format!("line {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(dir.join("note.txt"), content).unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "read": {
                  "kind": "file_read",
                  "capability": "file.read",
                  "base_dir": "."
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let output = tools
        .call_tool("read", &json!({"filePath": "note.txt", "start_line": 100}))
        .unwrap();

    assert_eq!(output["start_line"], json!(100));
    assert_eq!(output["end_line"], json!(350));
    let content = output["content"].as_str().unwrap();
    assert!(content.contains("00100| line 100"));
    assert!(content.contains("00350| line 350"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn file_read_limits_large_unscoped_reads_to_a_bounded_prefix() {
    let dir = temp_dir("air-tools-file-read-large-unscoped");
    let content = (1..=2200)
        .map(|line| format!("line {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(dir.join("note.txt"), content).unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "read": {
                  "kind": "file_read",
                  "capability": "file.read",
                  "base_dir": ".",
                  "max_bytes": 65536
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let output = tools
        .call_tool("read", &json!({"filePath": "note.txt"}))
        .unwrap();

    assert_eq!(output["start_line"], json!(1));
    assert_eq!(output["end_line"], json!(2000));
    assert_eq!(output["total_lines"], json!(2200));
    assert_eq!(output["range_limited_unscoped_read"], json!(true));
    assert!(output["truncation_hint"]
        .as_str()
        .is_some_and(|hint| hint.contains("offset+limit")));
    assert!(!output["content"].as_str().unwrap().contains("line 2200"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn file_read_repeated_same_range_returns_no_new_information() {
    let dir = temp_dir("air-tools-file-read-repeat-range");
    fs::write(dir.join("note.txt"), "zero\none\ntwo\nthree\n").unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "read": {
                  "kind": "file_read",
                  "capability": "file.read",
                  "base_dir": "."
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let first = tools
        .call_tool(
            "read",
            &json!({"filePath": "note.txt", "offset": 0, "limit": 3}),
        )
        .unwrap();
    assert_eq!(first["no_new_information"], Value::Null);

    let repeated = tools
        .call_tool(
            "read",
            &json!({"filePath": "note.txt", "offset": 0, "limit": 3}),
        )
        .unwrap();

    assert_eq!(repeated["no_new_information"], json!(true));
    assert_eq!(repeated["already_read"], json!(true));
    assert_eq!(repeated["content"], Value::Null);
    assert_eq!(repeated["covered_by"]["start_line"], json!(1));
    assert_eq!(repeated["covered_by"]["end_line"], json!(3));

    let narrower = tools
        .call_tool(
            "read",
            &json!({"filePath": "note.txt", "offset": 1, "limit": 1}),
        )
        .unwrap();
    assert_eq!(narrower["no_new_information"], Value::Null);
    assert_eq!(narrower["content"], json!("00002| one"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn file_read_accepts_start_line_limit() {
    let dir = temp_dir("air-tools-file-read-start-line-limit");
    fs::write(dir.join("note.txt"), "zero\none\ntwo\nthree\n").unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "read": {
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
            "read",
            &json!({"filePath": "note.txt", "start_line": 2, "limit": 2}),
        )
        .unwrap();

    assert_eq!(output["start_line"], json!(2));
    assert_eq!(output["end_line"], json!(3));
    assert_eq!(output["content"], json!("00002| one\n00003| two"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn file_read_rejects_mixed_opencode_and_line_range_inputs() {
    let dir = temp_dir("air-tools-file-read-redundant-range-inputs");
    fs::write(dir.join("note.txt"), "zero\none\ntwo\nthree\nfour\n").unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "read": {
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
            "read",
            &json!({
                "filePath": "note.txt",
                "offset": 1,
                "start_line": 2,
                "end_line": 4,
                "limit": 2
            }),
        )
        .unwrap_err();

    assert!(error
        .to_string()
        .contains("input.offset cannot be combined"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn file_search_repeated_query_returns_no_new_information() {
    let dir = temp_dir("air-tools-file-search-repeat-query");
    fs::write(dir.join("note.txt"), "alpha\nneedle\nomega\n").unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "grep": {
                  "kind": "file_search",
                  "capability": "file.read",
                  "base_dir": ".",
                  "max_matches": 8,
                  "max_bytes": 4096
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let first = tools
        .call_tool("grep", &json!({"path": "note.txt", "pattern": "needle"}))
        .unwrap();
    assert_eq!(first["match_count"], json!(1));
    assert_eq!(first["no_new_information"], Value::Null);

    let repeated = tools
        .call_tool("grep", &json!({"path": "note.txt", "pattern": "needle"}))
        .unwrap();

    assert_eq!(repeated["no_new_information"], json!(true));
    assert_eq!(repeated["already_seen"], json!(true));
    assert_eq!(repeated["matches"], Value::Null);
    assert_eq!(repeated["match_count"], json!(1));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn file_search_no_matches_returns_actionable_hint() {
    let dir = temp_dir("air-tools-file-search-no-matches");
    fs::write(dir.join("note.txt"), "alpha\nomega\n").unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "grep": {
                  "kind": "file_search",
                  "capability": "file.read",
                  "base_dir": ".",
                  "max_matches": 8,
                  "max_bytes": 4096
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let output = tools
        .call_tool("grep", &json!({"path": "note.txt", "pattern": "needle"}))
        .unwrap();

    assert_eq!(output["match_count"], json!(0));
    assert_eq!(output["no_matches"], json!(true));
    assert!(output["search_hint"]
        .as_str()
        .is_some_and(|hint| hint.contains("broaden")));
    assert_eq!(
        output["artifacts"][0]["metadata"]["no_matches"],
        json!(true)
    );
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
fn file_search_accepts_opencode_grep_aliases() {
    let dir = temp_dir("air-tools-file-search-opencode");
    fs::write(dir.join("note.txt"), "alpha\nneedle\nomega\n").unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "grep": {
                  "kind": "file_search",
                  "capability": "file.read",
                  "base_dir": ".",
                  "max_matches": 8,
                  "max_context_lines": 4
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let output = tools
        .call_tool(
            "grep",
            &json!({"pattern": "needle", "maxMatches": 4, "contextLines": 1}),
        )
        .unwrap();

    assert!(output["path"]
        .as_str()
        .is_some_and(|path| path.ends_with("air-tools-file-search-opencode")
            || path.contains("air-tools-file-search-opencode-")));
    assert!(output["base_path"]
        .as_str()
        .is_some_and(|path| path.ends_with("air-tools-file-search-opencode")
            || path.contains("air-tools-file-search-opencode-")));
    assert_eq!(output["match_count"], json!(1));
    assert_eq!(output["matches"][0]["path"], json!("note.txt"));
    assert_eq!(output["matches"][0]["line_number"], json!(2));
    assert_eq!(output["matches"][0]["before"][0]["line"], json!("alpha"));
    let content = output["artifacts"][0]["content"].as_str().unwrap();
    assert!(content.contains("note.txt:\n"));
    assert!(content.contains("  Line 1: alpha"));
    assert!(content.contains("  Line 2: needle"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn file_search_treats_exact_include_as_scope() {
    let dir = temp_dir("air-tools-file-search-include-scope");
    fs::write(dir.join("note.txt"), "alpha\nuse serde_json::Value;\n").unwrap();
    fs::write(
        dir.join("other.txt"),
        "use serde_json::Map;\nshould not be returned\n",
    )
    .unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "grep": {
                  "kind": "file_search",
                  "capability": "file.read",
                  "base_dir": ".",
                  "max_matches": 8
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let output = tools
        .call_tool(
            "grep",
            &json!({"pattern": "use serde_json", "include": "note.txt"}),
        )
        .unwrap();

    assert!(output["path"]
        .as_str()
        .is_some_and(|path| path.ends_with("note.txt")));
    assert_eq!(output["directory"], json!(false));
    assert_eq!(output["match_count"], json!(1));
    assert_eq!(
        output["matches"][0]["line"],
        json!("use serde_json::Value;")
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn file_search_treats_include_glob_as_file_filter() {
    let dir = temp_dir("air-tools-file-search-include-glob");
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(dir.join("src").join("lib.rs"), "fn target() {}\n").unwrap();
    fs::write(dir.join("src").join("note.txt"), "fn target() {}\n").unwrap();
    fs::write(dir.join("README.md"), "fn target() {}\n").unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "grep": {
                  "kind": "file_search",
                  "capability": "file.read",
                  "base_dir": ".",
                  "max_matches": 8
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let output = tools
        .call_tool("grep", &json!({"pattern": "fn target", "include": "*.rs"}))
        .unwrap();

    assert_eq!(output["directory"], json!(true));
    assert_eq!(output["include_glob"], json!("*.rs"));
    assert_eq!(output["match_count"], json!(1));
    assert_eq!(output["matches"][0]["path"], json!("src/lib.rs"));
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

    assert_eq!(output["content"], json!("00002| two\n00003| three"));
    assert_eq!(output["content_format"], json!("line_numbered"));
    assert_eq!(output["line_numbers"], json!(true));
    assert_eq!(
        output["artifacts"][0]["metadata"]["line_numbers"],
        json!(true)
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn file_read_defaults_to_numbered_content() {
    let dir = temp_dir("air-tools-file-read-numbered-default");
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
        .call_tool("file.read", &json!({"path": "note.txt"}))
        .unwrap();

    assert_eq!(
        output["content"],
        json!("00001| one\n00002| two\n00003| three")
    );
    assert_eq!(output["content_format"], json!("line_numbered"));
    assert_eq!(output["line_numbers"], json!(true));
    assert_eq!(output["line_numbers_defaulted"], json!(true));
    assert_eq!(
        output["artifacts"][0]["metadata"]["line_numbers_defaulted"],
        json!(true)
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn file_read_does_not_truncate_only_because_line_numbers_expand_output() {
    let dir = temp_dir("air-tools-file-read-numbered-expanded");
    let content = (1..=20).map(|_| "x").collect::<Vec<_>>().join("\n");
    assert!(content.len() <= 64);
    fs::write(dir.join("note.txt"), content).unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "file.read": {
                  "kind": "file_read",
                  "capability": "file.read",
                  "base_dir": ".",
                  "max_bytes": 64
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let output = tools
        .call_tool("file.read", &json!({"path": "note.txt"}))
        .unwrap();

    assert_eq!(output["truncated"], json!(false));
    assert_eq!(output["full_output_path"], json!(null));
    assert!(output["content"].as_str().unwrap().contains("00020| x"));
    assert!(
        output["content"].as_str().unwrap().len() > 64,
        "line numbering may expand rendered output beyond the source-byte limit"
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn file_read_can_disable_default_line_numbers() {
    let dir = temp_dir("air-tools-file-read-plain-explicit");
    fs::write(dir.join("note.txt"), "one\ntwo\n").unwrap();
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
            &json!({"path": "note.txt", "line_numbers": false}),
        )
        .unwrap();

    assert_eq!(output["content"], json!("one\ntwo\n"));
    assert_eq!(output["content_format"], json!("plain"));
    assert_eq!(output["line_numbers"], json!(false));
    assert_eq!(output["line_numbers_defaulted"], json!(false));
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

    assert_eq!(output["content"], json!("00002| two\n00003| three"));
    assert_eq!(output["content_format"], json!("line_numbered"));
    assert_eq!(output["start_line"], json!(2));
    assert_eq!(output["end_line"], json!(3));
    assert_eq!(output["line_numbers"], json!(true));
    assert_eq!(output["line_numbers_defaulted"], json!(true));
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
fn file_write_accepts_opencode_file_path_alias() {
    let dir = temp_dir("air-tools-file-write-file-path");
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "write": {
                  "kind": "file_write",
                  "capability": "file.write",
                  "base_dir": ".",
                  "create_dirs": true,
                  "allow_overwrite": true
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let output = tools
        .call_tool(
            "write",
            &json!({"filePath": "src/todo_tools.rs", "content": "pub fn todo() {}\n"}),
        )
        .unwrap();

    assert_eq!(
        fs::read_to_string(dir.join("src/todo_tools.rs")).unwrap(),
        "pub fn todo() {}\n"
    );
    assert_eq!(output["created"], json!(true));
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

    assert!(error
        .to_string()
        .contains("input.path is outside configured base_dir"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn file_write_accepts_absolute_path_inside_base_dir() {
    let dir = temp_dir("air-tools-file-write-absolute");
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
    let path = dir.join("site/index.html");

    let output = tools
        .call_tool(
            "file.write",
            &json!({"path": path, "content": "<h1>AIR</h1>"}),
        )
        .unwrap();

    assert_eq!(fs::read_to_string(path).unwrap(), "<h1>AIR</h1>");
    assert_eq!(output["created"], json!(true));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn file_edit_replaces_unique_string() {
    let dir = temp_dir("air-tools-file-edit");
    fs::write(dir.join("note.txt"), "hello AIR\n").unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
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
    assert_eq!(output["post_edit_snippets"][0]["path"], json!("note.txt"));
    assert_eq!(
        output["post_edit_snippets"][0]["content"],
        json!("00001| hello agent IR")
    );
    assert_eq!(output["artifacts"][0]["content"], output["diff"]);
    assert_eq!(output["artifacts"][0]["kind"], json!("file_edit"));
    assert_eq!(tools.tool_capability("file.edit"), Some("file.write"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn file_edit_accepts_absolute_path_inside_base_dir() {
    let dir = temp_dir("air-tools-file-edit-absolute");
    let path = dir.join("note.txt");
    fs::write(&path, "hello AIR\n").unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
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

    let output = tools
        .call_tool(
            "file.edit",
            &json!({"path": path, "old_string": "AIR", "new_string": "agent IR"}),
        )
        .unwrap();

    assert_eq!(fs::read_to_string(path).unwrap(), "hello agent IR\n");
    assert_eq!(output["replacements"], json!(1));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn file_edit_accepts_partial_single_line_code_anchor() {
    let dir = temp_dir("air-tools-file-edit-partial-code-line");
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(
        dir.join("src/lib.rs"),
        "impl Tools {\n    fn tool_capability(&self, name: &str) -> Option<&str> {\n        None\n    }\n}\n",
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
        .call_tool("file.read", &json!({"path": "src/lib.rs"}))
        .unwrap();

    let output = tools
        .call_tool(
            "file.edit",
            &json!({
                "path": "src/lib.rs",
                "oldString": "    fn tool_capability",
                "newString": "    fn capability_for_tool"
            }),
        )
        .unwrap();

    assert_eq!(output["success"], json!(true));
    assert_eq!(output["applied"], json!(true));
    assert!(fs::read_to_string(dir.join("src/lib.rs"))
        .unwrap()
        .contains("fn capability_for_tool"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn file_edit_allows_replace_all_for_single_identifier_anchor() {
    let dir = temp_dir("air-tools-file-edit-replace-all-identifier");
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(
        dir.join("src/lib.rs"),
        "fn demo(chars: usize) -> usize {\n    let total = chars + 1;\n    total\n}\n",
    )
    .unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "file.edit": {
                  "kind": "file_edit",
                  "capability": "file.write",
                  "base_dir": "."
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let output = tools
        .call_tool(
            "file.edit",
            &json!({
                "path": "src/lib.rs",
                "oldString": "chars",
                "newString": "payload_chars",
                "replaceAll": true
            }),
        )
        .unwrap();

    assert_eq!(output["success"], json!(true));
    assert_eq!(output["applied"], json!(true));
    assert_eq!(output["replacements"], json!(2));
    assert_eq!(
        fs::read_to_string(dir.join("src/lib.rs")).unwrap(),
        "fn demo(payload_chars: usize) -> usize {\n    let total = payload_chars + 1;\n    total\n}\n"
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn file_edit_reports_missing_old_string_as_structured_validation_failure() {
    let dir = temp_dir("air-tools-file-edit-missing-old-string");
    fs::write(dir.join("note.rs"), "fn demo() {}\n").unwrap();
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
        .call_tool("file.read", &json!({"path": "note.rs"}))
        .unwrap();

    let output = tools
        .call_tool("file.edit", &json!({"filePath": "note.rs"}))
        .unwrap();

    assert_eq!(output["success"], json!(false));
    assert_eq!(output["applied"], json!(false));
    assert_eq!(output["diagnostics"][0]["field"], json!("old_string"));
    assert!(output["diagnostics"][0]["message"]
        .as_str()
        .unwrap()
        .contains("old_string"));
    assert_eq!(
        fs::read_to_string(dir.join("note.rs")).unwrap(),
        "fn demo() {}\n"
    );
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
fn file_edit_applies_multiple_edits() {
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
    assert_eq!(output["post_edit_snippets"].as_array().unwrap().len(), 1);
    assert!(output["post_edit_snippets"][0]["content"]
        .as_str()
        .unwrap()
        .contains("00001| title: ready"));
    assert!(output["post_edit_snippets"][0]["content"]
        .as_str()
        .unwrap()
        .contains("00003| owner: agent"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn file_edit_accepts_opencode_style_multi_edit_fields() {
    let dir = temp_dir("air-tools-file-edit-opencode-multiple-ops");
    fs::write(dir.join("note.txt"), "AIR AIR\nstatus: todo\n").unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "read": {
                  "kind": "file_read",
                  "capability": "file.read",
                  "base_dir": "."
                },
                "edit": {
                  "kind": "file_edit",
                  "capability": "file.write",
                  "base_dir": "."
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();
    tools
        .call_tool("read", &json!({"filePath": "note.txt"}))
        .unwrap();

    let output = tools
        .call_tool(
            "edit",
            &json!({
                "filePath": "note.txt",
                "edits": [
                    {
                        "oldString": "AIR",
                        "newString": "Agent",
                        "replaceAll": true
                    },
                    {
                        "oldString": "status: todo",
                        "newString": "status: done"
                    }
                ]
            }),
        )
        .unwrap();

    assert_eq!(output["success"], json!(true));
    assert_eq!(output["edit_count"], json!(2));
    assert_eq!(output["replacements"], json!(3));
    assert_eq!(
        fs::read_to_string(dir.join("note.txt")).unwrap(),
        "Agent Agent\nstatus: done\n"
    );
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

    let output = tools
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
        .unwrap();

    assert_eq!(output["success"], json!(false));
    assert_eq!(output["applied"], json!(false));
    assert_eq!(output["diagnostics"][0]["field"], json!("old_string"));
    assert!(output["diagnostics"][0]["message"]
        .as_str()
        .unwrap()
        .contains("input.edits[1].old_string was not found"));
    assert_eq!(
        fs::read_to_string(dir.join("note.txt")).unwrap(),
        "alpha\nbeta\ngamma\n"
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn file_edit_auto_supports_line_trimmed_match_strategy() {
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

    let output = tools
        .call_tool(
            "file.edit",
            &json!({
                "path": "note.txt",
                "old_string": "function demo() {\nreturn \"before\";\n}",
                "new_string": "function demo() {\n    return \"after\";\n}"
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
fn file_edit_supports_explicit_context_aware_match_strategy() {
    let dir = temp_dir("air-tools-file-edit-context-aware");
    fs::write(
        dir.join("note.txt"),
        "section {\n    keep this\n    actual change\n}\n",
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

    let output = tools
        .call_tool(
            "file.edit",
            &json!({
                "path": "note.txt",
                "old_string": "section {\n    keep this\n    stale middle\n}",
                "new_string": "section {\n    keep this\n    updated change\n}",
                "match_strategy": "context_aware"
            }),
        )
        .unwrap();

    assert_eq!(output["match_strategy"], json!("context_aware"));
    assert_eq!(
        fs::read_to_string(dir.join("note.txt")).unwrap(),
        "section {\n    keep this\n    updated change\n}\n"
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

    let error_str = error.to_string();
    assert!(error_str.contains("input.match_strategy"));
    assert!(error_str.contains("block_anchor"));
    assert!(error_str.contains("multi_occurrence"));
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
fn file_edit_requires_replace_all_for_multiple_matches() {
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
                  "base_dir": "."
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
            &json!({"path": "note.txt", "old_string": "AIR", "new_string": "Agent"}),
        )
        .unwrap();
    assert_eq!(output["success"], json!(false));
    assert_eq!(output["applied"], json!(false));
    assert!(output["diagnostics"][0]["message"]
        .as_str()
        .unwrap()
        .contains("matched 2 times"));

    let output = tools
        .call_tool(
            "file.edit",
            &json!({
                "path": "note.txt",
                "old_string": "AIR",
                "new_string": "Agent",
                "replaceAll": true
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

    assert_eq!(output["content"], json!("00002| two\n00003| three"));
    assert_eq!(output["content_format"], json!("line_numbered"));
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

    assert_eq!(
        output["content"],
        json!("00002| before\n00003| target symbol\n00004| after")
    );
    assert_eq!(output["content_format"], json!("line_numbered"));
    assert_eq!(output["start_line"], json!(2));
    assert_eq!(output["end_line"], json!(4));
    assert_eq!(output["match_line"], json!(3));
    assert_eq!(
        output["artifacts"][0]["metadata"]["contains"],
        json!("target")
    );
    assert_eq!(output["occurrence"], json!(1));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn file_read_truncates_unscoped_large_reads_with_range_hint() {
    let dir = temp_dir("air-tools-file-read-unscoped-large");
    let content = (1..=200)
        .map(|line| format!("line {line:03} {}", "x".repeat(30)))
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(dir.join("large.txt"), content).unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "file.read": {
                  "kind": "file_read",
                  "capability": "file.read",
                  "base_dir": ".",
                  "max_bytes": 512
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let output = tools
        .call_tool(
            "file.read",
            &json!({"path": "large.txt", "line_numbers": true}),
        )
        .unwrap();

    assert_eq!(output["truncated"], json!(true));
    assert_eq!(output["unscoped_read"], json!(true));
    assert!(output["bytes"].as_u64().unwrap() > 512);
    assert_eq!(output["content_format"], json!("line_numbered"));
    assert!(output["content"].as_str().unwrap().len() > 512);
    assert!(output["content"]
        .as_str()
        .unwrap()
        .contains("00001| line 001"));
    assert!(output["truncation_hint"]
        .as_str()
        .unwrap()
        .contains("start_line/end_line"));
    let full_output_path = output["full_output_path"].as_str().unwrap();
    assert!(full_output_path.ends_with(".log"), "{full_output_path}");
    assert!(Path::new(full_output_path)
        .starts_with(dir.canonicalize().unwrap().join(".air").join("tool-output")));
    assert!(fs::read_to_string(full_output_path)
        .unwrap()
        .contains("line 200"));
    assert!(output.get("numbered_content").is_none());
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn file_search_saves_full_returned_preview_when_content_is_truncated() {
    let dir = temp_dir("air-tools-file-search-full-output");
    fs::write(
        dir.join("note.txt"),
        (1..=40)
            .map(|line| format!("needle line {line:02} {}", "x".repeat(40)))
            .collect::<Vec<_>>()
            .join("\n"),
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
                  "max_bytes": 128,
                  "max_matches": 40
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

    assert_eq!(output["truncated"], json!(true));
    let full_output_path = output["full_output_path"].as_str().unwrap();
    assert!(full_output_path.ends_with(".log"), "{full_output_path}");
    assert!(fs::read_to_string(full_output_path)
        .unwrap()
        .contains("needle line 40"));
    assert!(output["truncation_hint"]
        .as_str()
        .unwrap()
        .contains("Full returned search preview saved"));
    assert_eq!(
        output["artifacts"][0]["metadata"]["full_output_path"],
        json!(full_output_path)
    );
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

    assert_eq!(
        output["content"],
        json!("00003| before\n00004| target second\n00005| after")
    );
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
fn repo_files_glob_no_matches_returns_empty_listing() {
    let dir = temp_dir("air-tools-repo-files-glob-no-matches");
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(dir.join("src/lib.rs"), "pub fn alpha() {}\n").unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "glob": {
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
        .call_tool("glob", &json!({"pattern": "**/*edit*loop*test*"}))
        .unwrap();

    assert_eq!(output["files"], json!([]));
    assert_eq!(output["file_count"], json!(0));
    assert_eq!(output["no_matches"], json!(true));
    assert!(output["search_hint"]
        .as_str()
        .is_some_and(|hint| hint.contains("different glob")));
    assert_eq!(output["truncated"], json!(false));
    assert_eq!(output["glob"], json!("**/*edit*loop*test*"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn repo_search_no_matches_returns_actionable_hint() {
    let dir = temp_dir("air-tools-repo-search-no-matches");
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
                  "max_matches": 8,
                  "max_bytes": 4096
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let output = tools
        .call_tool("repo.search", &json!({"query": "does_not_exist"}))
        .unwrap();

    assert_eq!(output["match_count"], json!(0));
    assert_eq!(output["no_matches"], json!(true));
    assert!(output["search_hint"]
        .as_str()
        .is_some_and(|hint| hint.contains("broaden")));
    assert_eq!(
        output["artifacts"][0]["metadata"]["no_matches"],
        json!(true)
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn search_no_match_streak_surfaces_exhausted_search_direction() {
    let dir = temp_dir("air-tools-search-no-match-streak");
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(dir.join("src/lib.rs"), "pub fn alpha() {}\n").unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "glob": {
                  "kind": "repo_files",
                  "capability": "code.read",
                  "repo_dir": ".",
                  "max_files": 10
                },
                "grep": {
                  "kind": "file_search",
                  "capability": "file.read",
                  "base_dir": ".",
                  "max_matches": 8,
                  "max_bytes": 4096
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let first = tools
        .call_tool("glob", &json!({"pattern": "**/*.tsx"}))
        .unwrap();
    assert_eq!(first["no_match_streak"], json!(1));

    let second = tools
        .call_tool("grep", &json!({"pattern": "does_not_exist"}))
        .unwrap();
    assert_eq!(second["no_match_streak"], json!(2));
    assert!(second["search_hint"]
        .as_str()
        .is_some_and(|hint| hint.contains("Several recent searches returned no matches")));

    let hit = tools
        .call_tool("grep", &json!({"pattern": "alpha"}))
        .unwrap();
    assert_eq!(hit["no_match_streak"], Value::Null);

    let after_reset = tools
        .call_tool("glob", &json!({"pattern": "**/*.jsx"}))
        .unwrap();
    assert_eq!(after_reset["no_match_streak"], json!(1));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn repo_files_combines_query_with_pattern_glob() {
    let dir = temp_dir("air-tools-repo-files-query-pattern-glob");
    fs::create_dir_all(dir.join("crates/air-cli/src")).unwrap();
    fs::create_dir_all(dir.join("crates/air-tools/src")).unwrap();
    fs::write(dir.join("crates/air-cli/src/code_agent.rs"), "tests\n").unwrap();
    fs::write(dir.join("crates/air-tools/src/file_tools.rs"), "edit\n").unwrap();
    fs::write(dir.join("crates/air-cli/src/edit_loop_tests.rs"), "edit\n").unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "glob": {
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
        .call_tool("glob", &json!({"pattern": "**/*test*", "query": "edit"}))
        .unwrap();

    assert_eq!(
        output["files"],
        json!(["crates/air-cli/src/edit_loop_tests.rs"])
    );
    assert_eq!(output["query"], json!("edit"));
    assert_eq!(output["query_source"], json!("query"));
    assert_eq!(output["glob"], json!("**/*test*"));
    assert_eq!(output["glob_source"], json!("pattern"));
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
fn code_agent_self_tools_validate_project_paths() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let config_path = root.join("examples/code-agent/tools.json");
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    assert_eq!(tools.tool_capability("question"), Some("code.read"));
    assert_eq!(tools.tool_capability("bash"), Some("code.test"));
    assert_eq!(tools.tool_capability("read"), Some("file.read"));
    assert_eq!(tools.tool_capability("glob"), Some("code.read"));
    assert_eq!(tools.tool_capability("grep"), Some("file.read"));
    assert_eq!(tools.tool_capability("edit"), Some("file.write"));
    assert_eq!(tools.tool_capability("write"), Some("file.write"));
    assert_eq!(tools.tool_capability("task"), Some("code.read"));
    assert_eq!(tools.tool_capability("webfetch"), Some("code.read"));
    assert_eq!(tools.tool_capability("todowrite"), Some("code.read"));
    assert_eq!(tools.tool_capability("todoread"), Some("code.read"));
    assert_eq!(tools.tool_capability("skill"), Some("code.read"));

    let error = tools
        .call_tool("bash", &json!({"description": "missing command"}))
        .unwrap_err();
    assert!(error.to_string().contains("input.command must be a string"));
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
    assert_eq!(output["symbols"][0]["line"], json!(1));
    assert_eq!(output["symbols"][0]["end_line"], json!(1));
    assert_eq!(output["artifacts"][0]["kind"], json!("repo_symbols"));
    assert_eq!(tools.tool_capability("repo.symbols"), Some("code.read"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn repo_symbols_treats_space_separated_fixed_query_as_any_term() {
    let dir = temp_dir("air-tools-repo-symbols-fixed-terms");
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(
        dir.join("src/lib.rs"),
        "fn call_file_write_tool() {}\nfn call_file_edit_tool() {}\nfn unrelated() {}\n",
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
        .call_tool("repo.symbols", &json!({"query": "file_write file_edit"}))
        .unwrap();

    let symbols = output["symbols"].as_array().unwrap();
    assert_eq!(output["effective_query"], json!("file_edit|file_write"));
    assert_eq!(symbols.len(), 2);
    assert_eq!(symbols[0]["name"], json!("call_file_write_tool"));
    assert_eq!(symbols[1]["name"], json!("call_file_edit_tool"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn repo_symbols_omits_local_variables_from_structure_map() {
    let dir = temp_dir("air-tools-repo-symbols-no-local-vars");
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(
        dir.join("src/lib.rs"),
        "pub fn alpha() {\n    let local_value = 1;\n}\nstruct Beta;\n",
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

    let output = tools.call_tool("repo.symbols", &json!({})).unwrap();

    let names = output["symbols"]
        .as_array()
        .unwrap()
        .iter()
        .map(|symbol| symbol["name"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(names, vec!["alpha", "Beta"]);
    let _ = fs::remove_dir_all(dir);
}

#[test]
#[ignore = "requires rust-analyzer installed on PATH"]
fn lsp_references_reuses_session_within_config_tools() {
    let dir = temp_dir("air-tools-lsp-session-cache");
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(
        dir.join("Cargo.toml"),
        "[package]\nname = \"air_tools_lsp_session_cache\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(
        dir.join("src/lib.rs"),
        "pub fn helper() -> usize { 1 }\n\npub fn caller() -> usize { helper() }\n",
    )
    .unwrap();
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "lsp": {
                  "kind": "rust_analyzer",
                  "capability": "code.read",
                  "root_dir": ".",
                  "command": "rust-analyzer",
                  "max_results": 20,
                  "max_diagnostics": 20,
                  "max_bytes": 65536
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    for _ in 0..2 {
        let output = tools
            .call_tool(
                "lsp",
                &json!({
                    "command": "references",
                    "path": "src/lib.rs",
                    "symbol": "helper",
                    "include_declaration": true
                }),
            )
            .unwrap();
        assert!(
            output["reference_count"].as_u64().unwrap_or_default() >= 2,
            "{output}"
        );
    }

    assert_eq!(tools.rust_analyzer_sessions.len(), 1);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn repo_symbols_reports_symbol_end_lines() {
    let dir = temp_dir("air-tools-repo-symbols-end-lines");
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(
        dir.join("src/lib.rs"),
        "fn alpha() {\n  beta();\n}\n\nfn beta() {\n  gamma();\n}\n\nfn gamma() {}\n",
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
        .call_tool("repo.symbols", &json!({"path": "src/lib.rs"}))
        .unwrap();

    let symbols = output["symbols"].as_array().unwrap();
    assert_eq!(symbols.len(), 3);
    assert_eq!(symbols[0]["name"], json!("alpha"));
    assert_eq!(symbols[0]["line"], json!(1));
    assert_eq!(symbols[0]["end_line"], json!(3));
    assert_eq!(symbols[1]["name"], json!("beta"));
    assert_eq!(symbols[1]["line"], json!(5));
    assert_eq!(symbols[1]["end_line"], json!(7));
    assert_eq!(symbols[2]["name"], json!("gamma"));
    assert_eq!(symbols[2]["line"], json!(9));
    assert_eq!(symbols[2]["end_line"], json!(9));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn repo_symbols_reports_multiline_rust_function_body_end_line() {
    let dir = temp_dir("air-tools-repo-symbols-multiline-fn-end-lines");
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(
        dir.join("src/lib.rs"),
        "fn alpha(\n  value: usize,\n) -> usize {\n  let next = value + 1;\n  next\n}\n\nfn beta() {}\n",
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
        .call_tool(
            "repo.symbols",
            &json!({"path": "src/lib.rs", "names": ["alpha"]}),
        )
        .unwrap();

    let symbols = output["symbols"].as_array().unwrap();
    assert_eq!(symbols.len(), 1);
    assert_eq!(symbols[0]["name"], json!("alpha"));
    assert_eq!(symbols[0]["line"], json!(1));
    assert_eq!(symbols[0]["end_line"], json!(6));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn repo_symbols_reports_pub_super_multiline_rust_function_body_end_line() {
    let dir = temp_dir("air-tools-repo-symbols-pub-super-multiline-fn-end-lines");
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(
        dir.join("src/lib.rs"),
        "pub(super) fn alpha(\n  name: &str,\n) -> Result<(), String> {\n  if name.is_empty() {\n    return Err(format!(\"missing {name}\"));\n  }\n  Ok(())\n}\n\nfn beta() {}\n",
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
        .call_tool(
            "repo.symbols",
            &json!({"path": "src/lib.rs", "names": ["alpha"]}),
        )
        .unwrap();

    let symbols = output["symbols"].as_array().unwrap();
    assert_eq!(symbols.len(), 1);
    assert_eq!(symbols[0]["name"], json!("alpha"));
    assert_eq!(symbols[0]["line"], json!(1));
    assert_eq!(symbols[0]["end_line"], json!(8));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn repo_symbols_reports_rust_impl_blocks_for_type_names() {
    let dir = temp_dir("air-tools-repo-symbols-rust-impl");
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(
        dir.join("src/lib.rs"),
        "pub struct Alpha {}\n\nimpl Alpha {\n  pub fn new() -> Self { Self {} }\n}\n\nimpl std::fmt::Display for Alpha {\n  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { todo!() }\n}\n\nimpl<T> Alpha<T> {\n  fn generic(&self) {}\n}\n\nfn beta() {}\n",
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
        .call_tool(
            "repo.symbols",
            &json!({"path": "src/lib.rs", "names": ["Alpha"]}),
        )
        .unwrap();

    let symbols = output["symbols"].as_array().unwrap();
    assert_eq!(symbols.len(), 4);
    assert_eq!(symbols[0]["kind"], json!("struct"));
    assert_eq!(symbols[0]["name"], json!("Alpha"));
    assert_eq!(symbols[1]["kind"], json!("impl"));
    assert_eq!(symbols[1]["name"], json!("Alpha"));
    assert_eq!(symbols[1]["line"], json!(3));
    assert_eq!(symbols[1]["end_line"], json!(5));
    assert_eq!(symbols[2]["kind"], json!("impl"));
    assert_eq!(symbols[2]["name"], json!("Alpha"));
    assert_eq!(symbols[2]["line"], json!(7));
    assert_eq!(symbols[2]["end_line"], json!(9));
    assert_eq!(symbols[3]["kind"], json!("impl"));
    assert_eq!(symbols[3]["name"], json!("Alpha"));
    assert_eq!(symbols[3]["line"], json!(11));
    assert_eq!(symbols[3]["end_line"], json!(13));
    assert!(output["artifacts"][0]["content"]
        .as_str()
        .unwrap()
        .contains("impl Alpha"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn repo_symbols_accepts_multiple_exact_names() {
    let dir = temp_dir("air-tools-repo-symbols-names");
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(
        dir.join("src/lib.rs"),
        "fn alpha() {}\nfn beta() {}\nfn gamma() {}\n",
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
        .call_tool(
            "repo.symbols",
            &json!({"path": "src/lib.rs", "names": ["gamma", "alpha"]}),
        )
        .unwrap();

    assert_eq!(output["names"], json!(["alpha", "gamma"]));
    let symbols = output["symbols"].as_array().unwrap();
    assert_eq!(symbols.len(), 2);
    assert_eq!(symbols[0]["name"], json!("alpha"));
    assert_eq!(symbols[0]["line"], json!(1));
    assert_eq!(symbols[0]["end_line"], json!(1));
    assert_eq!(symbols[1]["name"], json!("gamma"));
    assert_eq!(symbols[1]["line"], json!(3));
    assert_eq!(symbols[1]["end_line"], json!(3));
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
fn command_run_executes_configured_command() {
    let dir = temp_dir("air-tools-command-run");
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "test": {
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
        .call_tool("test", &json!({"command": "cargo_version"}))
        .unwrap();

    assert_eq!(output["success"], json!(true));
    assert!(output["log"].as_str().unwrap().contains("cargo"));
    assert_eq!(output["artifacts"][0]["kind"], json!("test_log"));
    assert_eq!(tools.tool_capability("test"), Some("code.test"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn bash_runs_shell_command() {
    let dir = temp_dir("air-tools-bash");
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "bash": {
                  "kind": "bash",
                  "capability": "code.test",
                  "cwd": ".",
                  "timeout_seconds": 10
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let output = tools
        .call_tool(
            "bash",
            &json!({"command": "printf 'hello from bash\\n'", "description": "smoke"}),
        )
        .unwrap();

    assert_eq!(output["success"], json!(true));
    assert_eq!(
        output["input_command"],
        json!("printf 'hello from bash\\n'")
    );
    assert_eq!(output["description"], json!("smoke"));
    assert_eq!(output["verification"], json!(false));
    assert!(output["log"].as_str().unwrap().contains("hello from bash"));

    let output = tools
        .call_tool(
            "bash",
            &json!({"command": "true", "description": "Run verification"}),
        )
        .unwrap();
    assert_eq!(output["verification"], json!(true));
    assert_eq!(tools.tool_capability("bash"), Some("code.test"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn bash_search_containing_format_is_not_verification() {
    let dir = temp_dir("air-tools-bash-format-search");
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "bash": {
                  "kind": "bash",
                  "capability": "code.test",
                  "cwd": ".",
                  "timeout_seconds": 10
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let output = tools
        .call_tool(
            "bash",
            &json!({
                "command": "printf 'validate_positive_usize(path, &format!(...)\\n'",
                "description": "Find all max_bytes validation lines"
            }),
        )
        .unwrap();

    assert_eq!(output["success"], json!(true));
    assert_eq!(output["verification"], json!(false));
    assert!(output["reported_exit_status"].is_null());

    let output = tools
        .call_tool(
            "bash",
            &json!({
                "command": "rg -n \"validate_max_bytes\" crates/air-tools/src/lib.rs",
                "description": "Verify all validate_max_bytes usages"
            }),
        )
        .unwrap();

    assert_eq!(output["success"], json!(false));
    assert_eq!(output["verification"], json!(false));

    let output = tools
        .call_tool(
            "bash",
            &json!({
                "command": "true",
                "description": "Run the fixture test after the edit"
            }),
        )
        .unwrap();

    assert_eq!(output["success"], json!(true));
    assert_eq!(output["verification"], json!(true));

    fs::write(dir.join("check.py"), "print('ok')\n").unwrap();
    let output = tools
        .call_tool(
            "bash",
            &json!({
                "command": "python3 -m py_compile check.py",
                "description": "Compile check"
            }),
        )
        .unwrap();

    assert_eq!(output["success"], json!(true));
    assert_eq!(output["verification"], json!(true));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn bash_uses_pipefail_for_pipelines() {
    let dir = temp_dir("air-tools-bash-pipefail");
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "bash": {
                  "kind": "bash",
                  "capability": "code.test",
                  "cwd": ".",
                  "timeout_seconds": 10
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let output = tools
        .call_tool(
            "bash",
            &json!({"command": "false | cat", "description": "Run verification"}),
        )
        .unwrap();

    assert_eq!(output["success"], json!(false));
    assert_eq!(output["status"], json!(1));
    assert_eq!(output["verification"], json!(true));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn bash_verification_honors_reported_exit_status() {
    let dir = temp_dir("air-tools-bash-reported-exit-status");
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "bash": {
                  "kind": "bash",
                  "capability": "code.test",
                  "cwd": ".",
                  "timeout_seconds": 10
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let output = tools
        .call_tool(
            "bash",
            &json!({
                "command": "false; echo \"EXIT: $?\"",
                "description": "Run verification"
            }),
        )
        .unwrap();

    assert_eq!(output["verification"], json!(true));
    assert_eq!(output["reported_exit_status"], json!(1));
    assert_eq!(output["success"], json!(false));
    assert_eq!(output["status"], json!(1));
    assert_eq!(
        output["artifacts"][0]["metadata"]["reported_exit_status"],
        json!(1)
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn bash_verification_honors_decorated_reported_exit_status() {
    let dir = temp_dir("air-tools-bash-decorated-reported-exit-status");
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "bash": {
                  "kind": "bash",
                  "capability": "code.test",
                  "cwd": ".",
                  "timeout_seconds": 10
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let output = tools
        .call_tool(
            "bash",
            &json!({
                "command": "false; echo \"---EXIT:$?---\"",
                "description": "Run verification"
            }),
        )
        .unwrap();

    assert_eq!(output["verification"], json!(true));
    assert_eq!(output["reported_exit_status"], json!(1));
    assert_eq!(output["success"], json!(false));
    assert_eq!(output["status"], json!(1));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn bash_cargo_test_no_run_is_not_completion_verification() {
    let dir = temp_dir("air-tools-bash-no-run-not-verification");
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "bash": {
                  "kind": "bash",
                  "capability": "code.test",
                  "cwd": ".",
                  "timeout_seconds": 10
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let output = tools
        .call_tool(
            "bash",
            &json!({
                "command": "cargo test -p air-tools --no-run 2>&1; echo \"EXIT: $?\"",
                "description": "Run verification"
            }),
        )
        .unwrap();

    assert_eq!(output["verification"], json!(false));
    assert_eq!(output["reported_exit_status"], Value::Null);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn todowrite_records_todos() {
    let dir = temp_dir("air-tools-todowrite");
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "todowrite": {
                  "kind": "todo_write",
                  "capability": "code.read"
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let output = tools
        .call_tool(
            "todowrite",
            &json!({
                "todos": [
                    {"content": "inspect target", "status": "completed", "priority": "high"},
                    {"content": "apply edit", "status": "pending"}
                ]
            }),
        )
        .unwrap();

    assert_eq!(output["todo_count"], json!(2));
    assert_eq!(output["todos"][0]["content"], json!("inspect target"));
    assert_eq!(output["artifacts"][0]["kind"], json!("todo_list"));
    assert_eq!(tools.tool_capability("todowrite"), Some("code.read"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn command_run_uses_single_configured_command_by_default() {
    let dir = temp_dir("air-tools-command-run-default-command");
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "test": {
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

    let output = tools.call_tool("test", &json!({})).unwrap();

    assert_eq!(output["success"], json!(true));
    assert!(output["log"].as_str().unwrap().contains("cargo"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn bash_drains_large_stdout_without_timing_out() {
    let dir = temp_dir("air-tools-bash-large-output");
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "bash": {
                  "kind": "bash",
                  "capability": "code.test",
                  "cwd": ".",
                  "timeout_seconds": 10,
                  "max_bytes": 1024
                }
              }
            }"#,
    );
    let mut tools = ConfigTools::from_file(config_path).unwrap();

    let output = tools
        .call_tool(
            "bash",
            &json!({"command": "python3 -c 'import sys; sys.stdout.write(\"diff-line\\\\n\" * 25000)'", "description": "Show large diff"}),
        )
        .unwrap();

    assert_eq!(output["success"], json!(true));
    assert_eq!(output["truncated"], json!(true));
    assert_eq!(output["bytes"], json!(1024));
    assert!(
        fs::read_to_string(output["full_log_path"].as_str().unwrap())
            .unwrap()
            .len()
            >= 200000
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn command_run_saves_full_log_when_truncated() {
    let dir = temp_dir("air-tools-command-run-truncated-log");
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "test": {
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
        .call_tool("test", &json!({"command": "long_log"}))
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
                "test": {
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
        .call_tool("test", &json!({"command": "tail_log"}))
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
                "test": {
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
        .call_tool("test", &json!({"command": "long_log"}))
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
                "test": {
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
        .call_tool("test", &json!({"command": "unit"}))
        .unwrap_err();

    assert!(error
        .to_string()
        .contains("tool test command unit is not configured"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn command_run_extracts_structured_diagnostics() {
    let dir = temp_dir("air-tools-command-run-diagnostics");
    let config_path = write_config(
        &dir,
        r#"{
              "tools": {
                "test": {
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

    let output = tools.call_tool("test", &json!({"command": "tsc"})).unwrap();

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
                "test": {
                  "kind": "command_run",
                  "capability": "code.test",
                  "cwd": ".",
                  "commands": {
                    "echo_test": ["node", "-e", "console.log(process.argv[1])", "{{ case_name }}"]
                  },
                  "parameters": {
                    "case_name": {
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
            "test",
            &json!({"command": "echo_test", "case_name": "module::test_name"}),
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
                "test": {
                  "kind": "command_run",
                  "capability": "code.test",
                  "cwd": ".",
                  "commands": {
                    "echo_test": ["node", "-e", "console.log(process.argv[1])", "{{ case_name }}"]
                  },
                  "parameters": {
                    "case_name": {
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
            "test",
            &json!({
                "command": "echo_test",
                "args": {"case_name": "module::test_name"}
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
                "test": {
                  "kind": "command_run",
                  "cwd": ".",
                  "commands": {
                    "echo_test": ["node", "-e", "console.log(process.argv[1])", "{{case_name}}"]
                  },
                  "parameters": {
                    "case_name": {
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
            "test",
            &json!({"command": "echo_test", "case_name": "--eval=bad"}),
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
