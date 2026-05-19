use crate::regression::{run_regression_suite, RegressionRunConfig, RegressionRunReport};
use anyhow::{bail, Context, Result};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

pub(crate) struct SelfPrepareOptions {
    pub(crate) finding: String,
    pub(crate) candidates: usize,
    pub(crate) out_dir: Option<PathBuf>,
}

pub(crate) struct SelfCompareOptions {
    pub(crate) finding: String,
    pub(crate) from: Vec<PathBuf>,
    pub(crate) out: Option<PathBuf>,
}

pub(crate) struct SelfCaptureOptions {
    pub(crate) finding: String,
    pub(crate) candidate: String,
    pub(crate) out_dir: Option<PathBuf>,
}

pub(crate) struct SelfFixOptions {
    pub(crate) finding: String,
    pub(crate) candidates: usize,
    pub(crate) skill: Option<String>,
    pub(crate) task: Option<String>,
    pub(crate) from: Vec<PathBuf>,
    pub(crate) out_dir: Option<PathBuf>,
    pub(crate) model_config: Option<PathBuf>,
    pub(crate) regression_file: Option<PathBuf>,
    pub(crate) evaluate: bool,
    pub(crate) keep_worktrees: bool,
}

#[derive(Debug, Serialize)]
struct SelfPrepareOutput {
    schema: &'static str,
    finding: String,
    candidates_dir: String,
    candidates: Vec<CandidateSlot>,
    next_commands: Vec<String>,
}

#[derive(Debug, Serialize)]
struct CandidateSlot {
    id: String,
    path: String,
    patch: String,
    eval_json: String,
}

#[derive(Debug, Serialize)]
struct SelfCaptureOutput {
    schema: &'static str,
    finding: String,
    candidate: String,
    path: String,
    patch: String,
    changed_files: Vec<String>,
    patch_bytes: usize,
    next_command: String,
}

#[derive(Debug, Serialize)]
struct SelfFixOutput {
    schema: &'static str,
    finding: String,
    status: String,
    target: String,
    candidates_dir: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    preflight_regression: Option<RegressionRunReport>,
    candidates: Vec<SelfFixCandidateOutput>,
}

#[derive(Debug, Serialize)]
struct SelfFixCandidateOutput {
    id: String,
    path: String,
    worktree: String,
    worktree_kept: bool,
    patch: Option<String>,
    changed_files: Vec<String>,
    patch_bytes: usize,
    generation_status: Option<i32>,
    generation: GenerationGate,
    eval_json: Option<String>,
    eval_status: Option<i32>,
}

#[derive(Debug, Clone, Serialize)]
struct GenerationGate {
    passed: bool,
    final_success: bool,
    verification_ran: bool,
    verification_passed: bool,
    status: Option<i32>,
    failure_reason: Option<String>,
    output_json: Option<String>,
}

#[derive(Debug, Serialize)]
struct SelfCompareOutput {
    schema: &'static str,
    finding: String,
    candidates: Vec<CandidateScore>,
    recommendation: String,
    report: Option<String>,
}

#[derive(Debug, Serialize)]
struct CandidateScore {
    id: String,
    path: String,
    patch: Option<String>,
    changed_files: usize,
    patch_bytes: usize,
    decision: String,
    score_delta: i64,
    generation_passed: bool,
    generation_final_success: bool,
    generation_verification_ran: bool,
    generation_verification_passed: bool,
    regression_passed: usize,
    regression_total: usize,
    regression_statuses: Vec<String>,
    tests_passed: bool,
    guard_passed: bool,
    blocked: usize,
    warnings: usize,
}

#[derive(Debug, Default)]
struct SelfFixContext {
    sections: Vec<String>,
}

struct WorktreeGuard {
    repo: PathBuf,
    worktree: PathBuf,
    keep: bool,
}

impl Drop for WorktreeGuard {
    fn drop(&mut self) {
        if self.keep {
            return;
        }
        let Some(path) = self.worktree.to_str() else {
            return;
        };
        let _ = Command::new("git")
            .args(["worktree", "remove", "--force", path])
            .current_dir(&self.repo)
            .status();
    }
}

impl SelfFixContext {
    fn push(&mut self, title: &str, body: String) {
        if !body.trim().is_empty() {
            self.sections.push(format!("## {title}\n\n{}", body.trim()));
        }
    }

    fn to_prompt(&self) -> String {
        if self.sections.is_empty() {
            return "No structured finding/regression context was discovered. Inspect repository improve outputs before editing.".to_string();
        }
        self.sections.join("\n\n")
    }
}

pub(crate) fn self_prepare(options: SelfPrepareOptions) -> Result<()> {
    let cwd = std::env::current_dir().context("resolve current directory")?;
    let count = options.candidates.clamp(1, 12);
    let base = absolutize(
        &cwd,
        options
            .out_dir
            .unwrap_or_else(|| PathBuf::from(".air/candidates")),
    )
    .join(&options.finding);
    fs::create_dir_all(&base).with_context(|| format!("create {}", base.display()))?;
    let mut candidates = Vec::new();
    let mut next_commands = Vec::new();
    for index in 1..=count {
        let id = format!("cand-{index}");
        let dir = base.join(&id);
        fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
        let readme = dir.join("README.md");
        if !readme.exists() {
            fs::write(
                &readme,
                format!(
                    "# AIR Candidate {id}\n\nFinding: `{}`\n\nCapture the current workspace diff into this candidate slot:\n\n```bash\ncargo run -p air-cli -- self capture {} {id}\n```\n\nThen evaluate it from the repository root:\n\n```bash\ncargo run -p air-cli -- improve evaluate {} --report {}/eval.md\n```\n\nSave the JSON output as `{}/eval.json` before running `air self compare`.\n",
                    options.finding,
                    options.finding,
                    options.finding,
                    dir.display(),
                    dir.display()
                ),
            )
            .with_context(|| format!("write {}", readme.display()))?;
        }
        let eval_json = dir.join("eval.json");
        candidates.push(CandidateSlot {
            id: id.clone(),
            path: dir.display().to_string(),
            patch: dir.join("patch.diff").display().to_string(),
            eval_json: eval_json.display().to_string(),
        });
        next_commands.push(format!(
            "air improve evaluate {} --report {}/eval.md",
            options.finding,
            dir.display()
        ));
    }
    let output = SelfPrepareOutput {
        schema: "air.self_prepare.v1",
        finding: options.finding,
        candidates_dir: base.display().to_string(),
        candidates,
        next_commands,
    };
    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}

pub(crate) fn self_capture(options: SelfCaptureOptions) -> Result<()> {
    let cwd = std::env::current_dir().context("resolve current directory")?;
    let candidate_dir = absolutize(
        &cwd,
        options
            .out_dir
            .unwrap_or_else(|| PathBuf::from(".air/candidates")),
    )
    .join(&options.finding)
    .join(&options.candidate);
    fs::create_dir_all(&candidate_dir)
        .with_context(|| format!("create {}", candidate_dir.display()))?;

    let (diff, changed_files) = render_workspace_candidate_diff(&cwd)?;
    if diff.trim().is_empty() {
        bail!(
            "no workspace diff to capture for candidate {}",
            options.candidate
        );
    }

    let patch = candidate_dir.join("patch.diff");
    let files_json = candidate_dir.join("changed_files.json");
    fs::write(&patch, &diff).with_context(|| format!("write {}", patch.display()))?;
    fs::write(&files_json, serde_json::to_string_pretty(&changed_files)?)
        .with_context(|| format!("write {}", files_json.display()))?;

    let output = SelfCaptureOutput {
        schema: "air.self_capture.v1",
        finding: options.finding.clone(),
        candidate: options.candidate.clone(),
        path: candidate_dir.display().to_string(),
        patch: patch.display().to_string(),
        changed_files,
        patch_bytes: diff.len(),
        next_command: format!(
            "air improve evaluate {} --report {}/eval.md",
            options.finding,
            candidate_dir.display()
        ),
    };
    fs::write(
        candidate_dir.join("candidate.json"),
        serde_json::to_string_pretty(&output)?,
    )
    .with_context(|| format!("write {}", candidate_dir.join("candidate.json").display()))?;
    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}

pub(crate) fn self_fix(options: SelfFixOptions) -> Result<()> {
    let cwd = std::env::current_dir().context("resolve current directory")?;
    let count = options.candidates.clamp(1, 12);
    let base = absolutize(
        &cwd,
        options
            .out_dir
            .clone()
            .unwrap_or_else(|| PathBuf::from(".air/candidates")),
    )
    .join(&options.finding);
    fs::create_dir_all(&base).with_context(|| format!("create {}", base.display()))?;
    let air_exe = std::env::current_exe().context("resolve current air executable")?;
    let target = self_fix_target(options.skill.as_deref());
    let verification_command = self_fix_verification_command(options.skill.as_deref());
    let task = options
        .task
        .clone()
        .unwrap_or_else(|| default_self_fix_task(&options.finding, options.skill.as_deref()));
    let context = build_self_fix_context(
        &cwd,
        &options.finding,
        &options.from,
        options.regression_file.as_deref(),
        options.skill.as_deref(),
    )?;
    let preflight_regression = if options.evaluate {
        Some(run_regression_suite(RegressionRunConfig {
            finding: Some(options.finding.clone()),
            file: options.regression_file.clone(),
            all: false,
            from: options
                .from
                .iter()
                .map(|path| absolutize(&cwd, path.clone()))
                .collect(),
            out_dir: base.join("preflight").join("regression"),
        })?)
    } else {
        None
    };
    if preflight_regression
        .as_ref()
        .is_some_and(regression_already_resolved)
    {
        let output = SelfFixOutput {
            schema: "air.self_fix.v1",
            finding: options.finding,
            status: "no_fix_needed".to_string(),
            target,
            candidates_dir: base.display().to_string(),
            message: Some(
                "The promoted regression already resolves on the current baseline; no candidate patch was generated."
                    .to_string(),
            ),
            preflight_regression,
            candidates: Vec::new(),
        };
        println!("{}", serde_json::to_string_pretty(&output)?);
        return Ok(());
    }
    let mut reject_feedback = Vec::new();
    let mut candidates = Vec::new();

    for index in 1..=count {
        let id = format!("cand-{index}");
        let candidate_dir = base.join(&id);
        fs::create_dir_all(&candidate_dir)
            .with_context(|| format!("create {}", candidate_dir.display()))?;
        let worktree = std::env::temp_dir().join(format!(
            "air-self-fix-{}-{}-{}",
            std::process::id(),
            options.finding,
            id
        ));
        if worktree.exists() {
            fs::remove_dir_all(&worktree)
                .with_context(|| format!("remove stale {}", worktree.display()))?;
        }
        run_git(
            &cwd,
            &[
                "worktree",
                "add",
                "--detach",
                worktree_str(&worktree)?,
                "HEAD",
            ],
        )
        .with_context(|| format!("create candidate worktree {}", worktree.display()))?;
        let _worktree_guard = WorktreeGuard {
            repo: cwd.clone(),
            worktree: worktree.clone(),
            keep: options.keep_worktrees,
        };
        bootstrap_candidate_worktree(&cwd, &worktree, &candidate_dir)
            .with_context(|| format!("bootstrap candidate worktree {}", worktree.display()))?;

        let artifact_dir = candidate_dir.join("artifact");
        let trace = candidate_dir.join("trace.jsonl");
        let task_prompt = candidate_task(
            &task,
            &options.finding,
            &id,
            &target,
            &context,
            &reject_feedback,
            &verification_command,
        );
        let mut generate = Command::new(&air_exe);
        generate
            .arg("run")
            .arg(&task_prompt)
            .arg("--mode")
            .arg("code")
            .arg("--top-k")
            .arg("0")
            .arg("--verification-command")
            .arg(&verification_command)
            .arg("--artifact-out")
            .arg(&artifact_dir)
            .arg("--trace-out")
            .arg(&trace)
            .current_dir(&worktree);
        if let Some(model_config) = &options.model_config {
            generate
                .arg("--model-config")
                .arg(absolutize(&cwd, model_config.clone()));
        }
        fs::write(candidate_dir.join("task.md"), &task_prompt)
            .with_context(|| format!("write {}", candidate_dir.join("task.md").display()))?;
        let generation = generate
            .status()
            .with_context(|| format!("run code-agent for {}", id))?;
        let generation_gate = read_generation_gate(&artifact_dir, generation.code());

        let (diff, changed_files) = render_workspace_candidate_diff(&worktree)?;
        let patch = candidate_dir.join("patch.diff");
        let files_json = candidate_dir.join("changed_files.json");
        let patch_path = if diff.trim().is_empty() {
            None
        } else {
            fs::write(&patch, &diff).with_context(|| format!("write {}", patch.display()))?;
            fs::write(&files_json, serde_json::to_string_pretty(&changed_files)?)
                .with_context(|| format!("write {}", files_json.display()))?;
            Some(patch.display().to_string())
        };

        let (eval_json, eval_status) = if options.evaluate {
            let eval_out = candidate_dir.join("evaluation");
            let eval_report = candidate_dir.join("eval.md");
            let mut evaluate = Command::new(&air_exe);
            evaluate.arg("improve").arg("--out-dir").arg(&eval_out);
            for root in &options.from {
                evaluate.arg("--from").arg(absolutize(&cwd, root.clone()));
            }
            evaluate.arg("evaluate").arg(&options.finding);
            if let Some(regression_file) = &options.regression_file {
                evaluate
                    .arg("--regression-file")
                    .arg(absolutize(&cwd, regression_file.clone()));
            }
            evaluate
                .arg("--report")
                .arg(&eval_report)
                .current_dir(&worktree);
            let status = evaluate
                .status()
                .with_context(|| format!("evaluate candidate {}", id))?;
            let generated_eval = eval_out
                .join("evaluations")
                .join(&options.finding)
                .join("eval.json");
            if generated_eval.exists() {
                let target_eval = candidate_dir.join("eval.json");
                fs::copy(&generated_eval, &target_eval).with_context(|| {
                    format!(
                        "copy {} to {}",
                        generated_eval.display(),
                        target_eval.display()
                    )
                })?;
                postprocess_candidate_eval(&target_eval, &generation_gate)?;
                if let Some(feedback) = rejection_feedback(&id, &target_eval)? {
                    reject_feedback.push(feedback);
                }
                (Some(target_eval.display().to_string()), status.code())
            } else {
                reject_feedback.push(format!(
                    "{id}: evaluation exited {:?} and did not produce eval.json",
                    status.code()
                ));
                (None, status.code())
            }
        } else {
            (None, None)
        };
        if diff.trim().is_empty() {
            reject_feedback.push(format!(
                "{id}: generation exited {:?} but produced no workspace diff",
                generation.code()
            ));
        }
        if !generation_gate.passed {
            reject_feedback.push(format!(
                "{id}: generation gate failed: final_success={}, verification_ran={}, verification_passed={}, reason={}",
                generation_gate.final_success,
                generation_gate.verification_ran,
                generation_gate.verification_passed,
                generation_gate
                    .failure_reason
                    .as_deref()
                    .unwrap_or("unknown")
            ));
        }

        let candidate = SelfFixCandidateOutput {
            id,
            path: candidate_dir.display().to_string(),
            worktree: worktree.display().to_string(),
            worktree_kept: options.keep_worktrees,
            patch: patch_path,
            changed_files,
            patch_bytes: diff.len(),
            generation_status: generation.code(),
            generation: generation_gate,
            eval_json,
            eval_status,
        };
        fs::write(
            candidate_dir.join("candidate.json"),
            serde_json::to_string_pretty(&candidate)?,
        )
        .with_context(|| format!("write {}", candidate_dir.join("candidate.json").display()))?;
        candidates.push(candidate);
    }

    let output = SelfFixOutput {
        schema: "air.self_fix.v1",
        finding: options.finding,
        status: "candidates_generated".to_string(),
        target,
        candidates_dir: base.display().to_string(),
        message: None,
        preflight_regression,
        candidates,
    };
    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}

pub(crate) fn self_compare(options: SelfCompareOptions) -> Result<()> {
    let cwd = std::env::current_dir().context("resolve current directory")?;
    let roots = if options.from.is_empty() {
        vec![cwd.join(".air/candidates").join(&options.finding)]
    } else {
        options
            .from
            .into_iter()
            .map(|path| absolutize(&cwd, path))
            .collect::<Vec<_>>()
    };
    let mut scores = Vec::new();
    for root in &roots {
        collect_candidate_scores(root, &mut scores)?;
    }
    scores.sort_by(|left, right| {
        decision_rank(&right.decision)
            .cmp(&decision_rank(&left.decision))
            .then_with(|| right.score_delta.cmp(&left.score_delta))
            .then_with(|| left.id.cmp(&right.id))
    });
    let recommendation = scores
        .iter()
        .find(|score| score.decision == "accept_candidate")
        .map(|score| format!("accept {}", score.id))
        .unwrap_or_else(|| "no acceptable candidate".to_string());
    let report = options
        .out
        .as_ref()
        .map(|path| absolutize(&cwd, path.to_path_buf()));
    let output = SelfCompareOutput {
        schema: "air.self_compare.v1",
        finding: options.finding,
        candidates: scores,
        recommendation,
        report: report.as_ref().map(|path| path.display().to_string()),
    };
    if let Some(path) = report {
        write_compare_report(&path, &output)?;
    }
    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}

fn collect_candidate_scores(root: &Path, scores: &mut Vec<CandidateScore>) -> Result<()> {
    if !root.exists() {
        return Ok(());
    }
    if root.is_file() {
        if root.file_name().and_then(|name| name.to_str()) == Some("eval.json") {
            scores.push(read_candidate_score(root)?);
        }
        return Ok(());
    }
    let direct = root.join("eval.json");
    if direct.exists() {
        scores.push(read_candidate_score(&direct)?);
    }
    for entry in fs::read_dir(root).with_context(|| format!("read {}", root.display()))? {
        let entry = entry.with_context(|| format!("read entry under {}", root.display()))?;
        if entry.path().is_dir() {
            let eval = entry.path().join("eval.json");
            if eval.exists() {
                scores.push(read_candidate_score(&eval)?);
            }
        }
    }
    Ok(())
}

fn regression_already_resolved(regression: &RegressionRunReport) -> bool {
    regression.total > 0
        && regression.failed == 0
        && regression
            .results
            .iter()
            .all(|result| result.passed && matches!(result.status.as_str(), "passed" | "resolved"))
}

fn read_candidate_score(path: &Path) -> Result<CandidateScore> {
    let value: Value = serde_json::from_slice(
        &fs::read(path).with_context(|| format!("read {}", path.display()))?,
    )
    .with_context(|| format!("parse {}", path.display()))?;
    let candidate_dir = path.parent().unwrap_or(Path::new("."));
    let id = candidate_dir
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("candidate")
        .to_string();
    Ok(CandidateScore {
        id,
        path: candidate_dir.display().to_string(),
        patch: candidate_patch(candidate_dir),
        changed_files: candidate_changed_file_count(candidate_dir)?,
        patch_bytes: candidate_patch_bytes(candidate_dir)?,
        decision: string_field(&value, "decision").unwrap_or_else(|| "unknown".to_string()),
        score_delta: value
            .get("score_delta")
            .and_then(Value::as_i64)
            .unwrap_or(0),
        generation_passed: value
            .pointer("/generation/passed")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        generation_final_success: value
            .pointer("/generation/final_success")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        generation_verification_ran: value
            .pointer("/generation/verification_ran")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        generation_verification_passed: value
            .pointer("/generation/verification_passed")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        regression_passed: value
            .pointer("/regression/passed")
            .and_then(Value::as_u64)
            .unwrap_or(0) as usize,
        regression_total: value
            .pointer("/regression/total")
            .and_then(Value::as_u64)
            .unwrap_or(0) as usize,
        regression_statuses: value
            .pointer("/regression_gate/statuses")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default(),
        tests_passed: value
            .pointer("/tests/passed")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        guard_passed: value
            .pointer("/guard/passed")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        blocked: value
            .pointer("/guard/blocked")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or(0),
        warnings: value
            .pointer("/guard/warnings")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or(0),
    })
}

fn read_generation_gate(artifact_dir: &Path, status: Option<i32>) -> GenerationGate {
    let output_json = artifact_dir.join("output.json");
    let value = read_json_value(&output_json).ok();
    let final_success = value
        .as_ref()
        .and_then(|value| value.pointer("/edit/final_success"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let verification_ran = value
        .as_ref()
        .and_then(|value| value.pointer("/edit/verdict/verification_ran"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let verification_passed = value
        .as_ref()
        .and_then(|value| value.pointer("/edit/verdict/verification_passed"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let failure_reason = value
        .as_ref()
        .and_then(|value| {
            value
                .pointer("/edit/failure_reason/message")
                .or_else(|| value.pointer("/edit/verdict/failure_reason/message"))
        })
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            if output_json.exists() {
                None
            } else {
                Some("generation did not produce artifact output.json".to_string())
            }
        });
    GenerationGate {
        passed: status == Some(0) && final_success && verification_ran && verification_passed,
        final_success,
        verification_ran,
        verification_passed,
        status,
        failure_reason,
        output_json: output_json
            .exists()
            .then(|| output_json.display().to_string()),
    }
}

fn postprocess_candidate_eval(path: &Path, generation: &GenerationGate) -> Result<()> {
    let mut value = read_json_value(path)?;
    let regression_causal_pass = value
        .pointer("/regression_gate/causal_pass")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let Some(object) = value.as_object_mut() else {
        return Ok(());
    };
    object.insert("generation".to_string(), json!(generation));
    let decision = object
        .get("decision")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string();
    if decision == "accept_candidate" && (!generation.passed || !regression_causal_pass) {
        object.insert(
            "decision".to_string(),
            Value::String("needs_review".to_string()),
        );
        let score_delta = object
            .get("score_delta")
            .and_then(Value::as_i64)
            .unwrap_or(0)
            - 100;
        object.insert("score_delta".to_string(), json!(score_delta));
        let mut reasons = Vec::new();
        if !generation.passed {
            reasons
                .push("generation gate did not prove final_success with verification".to_string());
        }
        if !regression_causal_pass {
            reasons.push("regression gate did not produce a causal pass".to_string());
        }
        object.insert(
            "candidate_gate".to_string(),
            json!({ "blocked_accept": reasons }),
        );
    }
    fs::write(path, serde_json::to_string_pretty(&value)?)
        .with_context(|| format!("write {}", path.display()))
}

fn write_compare_report(path: &Path, output: &SelfCompareOutput) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    let mut out = String::new();
    out.push_str("# AIR Candidate Comparison\n\n");
    out.push_str(&format!("- finding: `{}`\n", output.finding));
    out.push_str(&format!(
        "- recommendation: `{}`\n\n",
        output.recommendation
    ));
    out.push_str(
        "| Candidate | Patch | Files | Bytes | Decision | Score | Generation | Regression | Regression Status | Tests | Guard | Blocked | Warnings |\n",
    );
    out.push_str("|---|---|---:|---:|---|---:|---|---|---|---|---|---:|---:|\n");
    for candidate in &output.candidates {
        out.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} / verify:{} | {}/{} | {} | {} | {} | {} | {} |\n",
            candidate.id,
            candidate.patch.as_deref().unwrap_or(""),
            candidate.changed_files,
            candidate.patch_bytes,
            candidate.decision,
            candidate.score_delta,
            candidate.generation_passed,
            candidate.generation_verification_passed,
            candidate.regression_passed,
            candidate.regression_total,
            candidate.regression_statuses.join(", "),
            candidate.tests_passed,
            candidate.guard_passed,
            candidate.blocked,
            candidate.warnings
        ));
    }
    fs::write(path, out).with_context(|| format!("write {}", path.display()))
}

fn string_field(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_string)
}

fn decision_rank(decision: &str) -> u8 {
    match decision {
        "accept_candidate" => 3,
        "needs_review" => 2,
        "reject_candidate" => 1,
        _ => 0,
    }
}

fn candidate_patch(candidate_dir: &Path) -> Option<String> {
    let patch = candidate_dir.join("patch.diff");
    patch.exists().then(|| patch.display().to_string())
}

fn candidate_patch_bytes(candidate_dir: &Path) -> Result<usize> {
    let patch = candidate_dir.join("patch.diff");
    if !patch.exists() {
        return Ok(0);
    }
    Ok(fs::metadata(&patch)
        .with_context(|| format!("read metadata for {}", patch.display()))?
        .len() as usize)
}

fn candidate_changed_file_count(candidate_dir: &Path) -> Result<usize> {
    let files = candidate_dir.join("changed_files.json");
    if files.exists() {
        let values: Vec<String> = serde_json::from_slice(
            &fs::read(&files).with_context(|| format!("read {}", files.display()))?,
        )
        .with_context(|| format!("parse {}", files.display()))?;
        return Ok(values.len());
    }
    Ok(0)
}

fn render_workspace_candidate_diff(cwd: &Path) -> Result<(String, Vec<String>)> {
    let mut diff = run_git_allow_diff(cwd, &["diff", "--binary", "HEAD", "--"])?;
    let tracked_files = run_git(cwd, &["diff", "--name-only", "HEAD", "--"])?;
    let untracked_files = run_git(cwd, &["ls-files", "--others", "--exclude-standard"])?;
    let mut files = BTreeSet::new();
    for file in tracked_files.lines().chain(untracked_files.lines()) {
        let file = file.trim();
        if !file.is_empty() {
            files.insert(file.to_string());
        }
    }
    for file in untracked_files
        .lines()
        .map(str::trim)
        .filter(|file| !file.is_empty())
    {
        let new_file_diff =
            run_git_allow_diff(cwd, &["diff", "--no-index", "--", "/dev/null", file])?;
        if !new_file_diff.trim().is_empty() {
            if !diff.ends_with('\n') {
                diff.push('\n');
            }
            diff.push_str(&new_file_diff);
        }
    }
    Ok((diff, files.into_iter().collect()))
}

fn bootstrap_candidate_worktree(
    source: &Path,
    worktree: &Path,
    candidate_dir: &Path,
) -> Result<()> {
    let tracked_diff = run_git_allow_diff(source, &["diff", "--binary", "HEAD", "--"])?;
    if !tracked_diff.trim().is_empty() {
        let patch = candidate_dir.join("bootstrap.patch");
        fs::write(&patch, &tracked_diff).with_context(|| format!("write {}", patch.display()))?;
        let patch_arg = patch
            .to_str()
            .with_context(|| format!("non-utf8 patch path {}", patch.display()))?;
        run_git(worktree, &["apply", "--binary", patch_arg])
            .with_context(|| format!("apply {}", patch.display()))?;
    }

    let untracked_files = run_git(source, &["ls-files", "--others", "--exclude-standard"])?;
    for file in untracked_files
        .lines()
        .map(str::trim)
        .filter(|file| !file.is_empty())
    {
        copy_bootstrap_file(source, worktree, file)?;
    }

    let status = run_git(worktree, &["status", "--porcelain"])?;
    if status.trim().is_empty() {
        return Ok(());
    }
    run_git(worktree, &["add", "-A"])?;
    run_git(
        worktree,
        &[
            "-c",
            "user.name=AIR Self Fix",
            "-c",
            "user.email=air-self-fix@example.invalid",
            "commit",
            "-m",
            "AIR self-fix bootstrap",
        ],
    )?;
    Ok(())
}

fn copy_bootstrap_file(source: &Path, worktree: &Path, relative: &str) -> Result<()> {
    let source_file = source.join(relative);
    let target_file = worktree.join(relative);
    if let Some(parent) = target_file.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    fs::copy(&source_file, &target_file).with_context(|| {
        format!(
            "copy {} to {}",
            source_file.display(),
            target_file.display()
        )
    })?;
    Ok(())
}

fn run_git(cwd: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .with_context(|| format!("run git {}", args.join(" ")))?;
    if !output.status.success() {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn run_git_allow_diff(cwd: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .with_context(|| format!("run git {}", args.join(" ")))?;
    if !output.status.success() && output.status.code() != Some(1) {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn worktree_str(path: &Path) -> Result<&str> {
    path.to_str()
        .with_context(|| format!("non-utf8 worktree path {}", path.display()))
}

fn self_fix_target(skill: Option<&str>) -> String {
    skill
        .map(|skill| format!("skill:{skill}"))
        .unwrap_or_else(|| "runtime".to_string())
}

fn self_fix_verification_command(skill: Option<&str>) -> String {
    if skill.is_some() {
        "cargo test -p air-cli skill --no-fail-fast".to_string()
    } else {
        "cargo test -p air-cli bench --no-fail-fast".to_string()
    }
}

fn build_self_fix_context(
    cwd: &Path,
    finding: &str,
    roots: &[PathBuf],
    regression_file: Option<&Path>,
    skill: Option<&str>,
) -> Result<SelfFixContext> {
    let mut context = SelfFixContext::default();
    if let Some(skill) = skill {
        context.push(
            "Skill Scope",
            format!(
                "Target local skill package: `{skill}`. Prefer edits under `skills/{skill}/` unless the finding evidence points elsewhere. External MCP servers are out of scope."
            ),
        );
    }

    let search_roots = self_fix_context_roots(cwd, roots);
    let mut files = Vec::new();
    for root in &search_roots {
        collect_context_json_files(root, finding, &mut files, 24)?;
    }
    if let Some(path) = regression_file {
        files.push(absolutize(cwd, path.to_path_buf()));
    }
    files.sort();
    files.dedup();

    for file in files.into_iter().take(16) {
        let Ok(value) = read_json_value(&file) else {
            continue;
        };
        if let Some(section) = finding_context_from_value(finding, &file, &value) {
            context.push("Finding Evidence", section);
        }
        if let Some(section) = regression_context_from_value(finding, &file, &value) {
            context.push("Regression Expectation", section);
        }
        if let Some(section) = bench_run_context_from_value(&file, &value) {
            context.push("Benchmark Failure Detail", section);
        }
        if let Some(section) = evaluation_context_from_value(&file, &value) {
            context.push("Previous Evaluation Feedback", section);
        }
    }
    Ok(context)
}

fn self_fix_context_roots(cwd: &Path, roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut out = roots
        .iter()
        .map(|path| absolutize(cwd, path.clone()))
        .collect::<Vec<_>>();
    if out.is_empty() {
        for default in [".air/improve/latest", ".air/regressions"] {
            let path = cwd.join(default);
            if path.exists() {
                out.push(path);
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

fn collect_context_json_files(
    root: &Path,
    finding: &str,
    files: &mut Vec<PathBuf>,
    budget: usize,
) -> Result<()> {
    if files.len() >= budget || !root.exists() {
        return Ok(());
    }
    if root.is_file() {
        if context_json_file_matches(root, finding) {
            files.push(root.to_path_buf());
        }
        return Ok(());
    }
    for entry in fs::read_dir(root).with_context(|| format!("read {}", root.display()))? {
        if files.len() >= budget {
            break;
        }
        let entry = entry.with_context(|| format!("read entry under {}", root.display()))?;
        let path = entry.path();
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        if path.is_dir() {
            if matches!(
                file_name,
                ".git" | "node_modules" | ".venv" | ".cache" | "target"
            ) {
                continue;
            }
            collect_context_json_files(&path, finding, files, budget)?;
        } else if context_json_file_matches(&path, finding) {
            files.push(path);
        }
    }
    Ok(())
}

fn context_json_file_matches(path: &Path, finding: &str) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    matches!(
        name,
        "findings.json"
            | "suggested_regressions.json"
            | "eval.json"
            | "candidate.json"
            | "run.json"
    ) || name == format!("{}.json", finding.to_ascii_lowercase())
}

fn read_json_value(path: &Path) -> Result<Value> {
    let bytes = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    serde_json::from_slice(&bytes).with_context(|| format!("parse {}", path.display()))
}

fn finding_context_from_value(finding: &str, path: &Path, value: &Value) -> Option<String> {
    let item = value
        .get("findings")
        .and_then(Value::as_array)
        .and_then(|items| {
            items
                .iter()
                .find(|item| item.get("id").and_then(Value::as_str) == Some(finding))
        })?;
    let mut out = format!(
        "- preloaded_evidence: `{}` (excerpt below; do not read this path from the candidate worktree)\n",
        evidence_label(path)
    );
    push_json_field(&mut out, item, "category");
    push_json_field(&mut out, item, "summary");
    push_json_field(&mut out, item, "count");
    push_json_field(&mut out, item, "impact_score");
    push_json_field(&mut out, item, "priority_reason");
    push_json_field(&mut out, item, "recommended_change_type");
    push_json_field(&mut out, item, "expected_regression");
    if let Some(evidence) = item.get("evidence").and_then(Value::as_array) {
        out.push_str("- evidence:\n");
        for value in evidence.iter().take(8) {
            out.push_str(&format!("  - {}\n", compact_json(value)));
        }
    }
    Some(out)
}

fn regression_context_from_value(finding: &str, path: &Path, value: &Value) -> Option<String> {
    let item = if value.get("finding_id").and_then(Value::as_str) == Some(finding) {
        Some(value)
    } else {
        value
            .get("regressions")
            .and_then(Value::as_array)
            .and_then(|items| {
                items
                    .iter()
                    .find(|item| item.get("finding_id").and_then(Value::as_str) == Some(finding))
            })
    }?;
    let mut out = format!(
        "- preloaded_evidence: `{}` (excerpt below; do not read this path from the candidate worktree)\n",
        evidence_label(path)
    );
    push_json_field(&mut out, item, "kind");
    push_json_field(&mut out, item, "category");
    push_json_field(&mut out, item, "title");
    if let Some(fixture_hint) = item.get("fixture_hint") {
        out.push_str(&format!("- fixture_hint: {}\n", compact_json(fixture_hint)));
    }
    Some(out)
}

fn bench_run_context_from_value(path: &Path, value: &Value) -> Option<String> {
    let tasks = value.get("tasks").and_then(Value::as_array)?;
    let suite_path = value.get("suite_path").and_then(Value::as_str);
    let mut failing = tasks
        .iter()
        .filter(|task| task.get("pass").and_then(Value::as_bool) == Some(false))
        .take(4)
        .collect::<Vec<_>>();
    if failing.is_empty() {
        failing = tasks.iter().take(2).collect();
    }
    if failing.is_empty() {
        return None;
    }

    let mut out = format!(
        "- preloaded_evidence: `{}` (excerpt below; do not read this path from the candidate worktree)\n",
        evidence_label(path)
    );
    if let Some(suite) = value.get("suite").and_then(Value::as_str) {
        out.push_str(&format!("- suite: `{suite}`\n"));
    }
    if let Some(suite_path) = suite_path {
        out.push_str(&format!(
            "- suite_path: `{}`\n",
            prompt_readable_path(suite_path)
        ));
    }
    for task in failing {
        out.push_str("- task:\n");
        push_json_field_indented(&mut out, task, "id", 2);
        push_json_field_indented(&mut out, task, "changed_files", 2);
        push_json_field_indented(&mut out, task, "failure_reason", 2);
        push_json_field_indented(&mut out, task, "verdict", 2);
        if let (Some(suite_path), Some(task_id)) =
            (suite_path, task.get("id").and_then(Value::as_str))
        {
            if let Some(detail) = suite_task_context(suite_path, task_id) {
                out.push_str(&indent_lines(&detail, 2));
            }
        }
    }
    Some(out)
}

fn suite_task_context(suite_path: &str, task_id: &str) -> Option<String> {
    let suite_path = Path::new(suite_path);
    let suite: Value = read_json_value(suite_path).ok()?;
    let task = suite
        .get("tasks")
        .and_then(Value::as_array)?
        .iter()
        .find(|task| task.get("id").and_then(Value::as_str) == Some(task_id))?;
    let mut out = String::new();
    out.push_str(&format!("- suite_task_id: `{task_id}`\n"));
    push_json_field(&mut out, task, "prompt");
    push_json_field(&mut out, task, "fixture");
    push_json_field(&mut out, task, "verification");
    push_json_field(&mut out, task, "diff");
    Some(out)
}

fn indent_lines(value: &str, spaces: usize) -> String {
    let prefix = " ".repeat(spaces);
    value
        .lines()
        .map(|line| format!("{prefix}{line}\n"))
        .collect()
}

fn evaluation_context_from_value(path: &Path, value: &Value) -> Option<String> {
    let decision = value.get("decision").and_then(Value::as_str)?;
    if decision == "accept_candidate" {
        return None;
    }
    let mut out = format!(
        "- preloaded_evidence: `{}` (excerpt below; do not read this path from the candidate worktree)\n- decision: `{decision}`\n",
        evidence_label(path)
    );
    push_json_field(&mut out, value, "score_delta");
    if let Some(regression) = value.get("regression") {
        out.push_str(&format!(
            "- regression: passed={}, failed={}, total={}\n",
            regression
                .get("passed")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            regression
                .get("failed")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            regression.get("total").and_then(Value::as_u64).unwrap_or(0)
        ));
    }
    if let Some(tests) = value.get("tests") {
        out.push_str(&format!(
            "- tests_passed: {}\n",
            tests
                .get("passed")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        ));
    }
    if let Some(guard) = value.get("guard") {
        append_guard_feedback(&mut out, guard);
    }
    Some(out)
}

fn rejection_feedback(candidate: &str, eval_json: &Path) -> Result<Option<String>> {
    let value = read_json_value(eval_json)?;
    let decision = value
        .get("decision")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    if decision == "accept_candidate" {
        return Ok(None);
    }
    let mut out = format!("{candidate}: decision={decision}");
    if let Some(regression) = value.get("regression") {
        out.push_str(&format!(
            ", regression_failed={}",
            regression
                .get("failed")
                .and_then(Value::as_u64)
                .unwrap_or(0)
        ));
    }
    if let Some(tests) = value.get("tests") {
        out.push_str(&format!(
            ", tests_passed={}",
            tests
                .get("passed")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        ));
    }
    if let Some(guard) = value.get("guard") {
        let blocked = guard
            .get("blocked")
            .and_then(Value::as_array)
            .map(|items| items.iter().map(compact_json).collect::<Vec<_>>())
            .unwrap_or_default();
        if !blocked.is_empty() {
            out.push_str(&format!(", guard_blocked={}", blocked.join("; ")));
        }
    }
    Ok(Some(out))
}

fn evidence_label(path: &Path) -> String {
    let parts = path
        .components()
        .filter_map(|component| component.as_os_str().to_str())
        .collect::<Vec<_>>();
    let start = parts.len().saturating_sub(4);
    parts[start..].join("/")
}

fn prompt_readable_path(path: &str) -> String {
    if let Some(index) = path.find("skills/") {
        return path[index..].to_string();
    }
    if let Some(index) = path.find("crates/") {
        return path[index..].to_string();
    }
    path.to_string()
}

fn append_guard_feedback(out: &mut String, guard: &Value) {
    for (label, key) in [("guard_blocked", "blocked"), ("guard_warnings", "warnings")] {
        if let Some(items) = guard.get(key).and_then(Value::as_array) {
            if !items.is_empty() {
                out.push_str(&format!("- {label}:\n"));
                for item in items.iter().take(8) {
                    out.push_str(&format!("  - {}\n", compact_json(item)));
                }
            }
        }
    }
}

fn push_json_field(out: &mut String, value: &Value, field: &str) {
    if let Some(field_value) = value.get(field) {
        out.push_str(&format!("- {field}: {}\n", compact_json(field_value)));
    }
}

fn push_json_field_indented(out: &mut String, value: &Value, field: &str, spaces: usize) {
    if let Some(field_value) = value.get(field) {
        out.push_str(&" ".repeat(spaces));
        out.push_str(&format!("- {field}: {}\n", compact_json(field_value)));
    }
}

fn compact_json(value: &Value) -> String {
    match value {
        Value::String(value) => truncate_chars(value, 500),
        _ => serde_json::to_string(value)
            .unwrap_or_else(|_| "<unserializable>".to_string())
            .chars()
            .take(500)
            .collect(),
    }
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    let mut out = String::new();
    for (index, ch) in value.chars().enumerate() {
        if index >= max_chars {
            out.push_str("...");
            return out;
        }
        out.push(ch);
    }
    out
}

fn default_self_fix_task(finding: &str, skill: Option<&str>) -> String {
    if let Some(skill) = skill {
        return format!(
            "Implement a minimal, reviewable candidate improvement for AIR skill `{skill}` for self-improvement finding {finding}. Use the structured finding and regression context below as the source of truth: address the failure category, expected behavior, and evidence directly. Focus on the local skill package, prompts, profiles, tool policy, tests, docs, or regressions that govern this skill. Do not modify external MCP servers. Do not widen capabilities, edit skills.lock, or weaken verification/eval gates unless the change is explicitly required and justified in the patch."
        );
    }
    format!(
        "Implement a minimal, reviewable candidate fix for AIR runtime self-improvement finding {finding}. Use the structured finding and regression context below as the source of truth: address the failure category, expected behavior, and evidence directly. Change only source files needed for the fix, preserve eval integrity, and do not edit benchmark or regression expected outputs unless explicitly required."
    )
}

fn candidate_task(
    task: &str,
    finding: &str,
    candidate: &str,
    target: &str,
    context: &SelfFixContext,
    reject_feedback: &[String],
    verification_command: &str,
) -> String {
    let mut out = format!(
        "{task}\n\nCandidate: {candidate}\nFinding: {finding}\nTarget: {target}\n\n# Structured Context\n\n{}\n",
        context.to_prompt()
    );
    if !reject_feedback.is_empty() {
        out.push_str("\n# Feedback From Rejected Candidates\n\n");
        for feedback in reject_feedback.iter().rev().take(5).rev() {
            out.push_str(&format!("- {feedback}\n"));
        }
    }
    out.push_str("\n# Candidate Rules\n\n");
    out.push_str(
        "- Before the first edit, inspect the listed benchmark/regression/suite/task evidence.\n",
    );
    out.push_str("- The structured context excerpts above are already preloaded; do not call read tools on `target/generated/**` evidence paths or absolute paths from the source checkout.\n");
    out.push_str("- Treat benchmark suites, fixtures, regressions, and eval manifests as evidence and gates, not as fix targets.\n");
    out.push_str("- Do not edit protected eval paths such as `skills/**/benches/**`, `benches/**`, `regressions/**`, `.air/evals/**`, or `skills.lock`.\n");
    out.push_str("- Do not run or edit unrelated fixtures just because they are easy to fix.\n");
    out.push_str("- If the context lists required_changed_files or allowed_changed_files, keep the patch inside those files unless the evidence proves a runtime fix elsewhere is required.\n");
    out.push_str("- Benchmark `required_changed_files` are workdir-relative expectations for the tested task; do not satisfy them by editing repository benchmark fixtures.\n");
    out.push_str("- The candidate is only useful if it addresses the listed regression gate, not merely any failing test in the repository.\n\n");
    out.push_str(&format!(
        "- AIR will run this deterministic verification command before accepting your generation artifact: `{verification_command}`.\n\n"
    ));
    out.push_str("Return only after applying a concrete code patch that passes the deterministic verification command.");
    out
}

fn absolutize(cwd: &Path, path: PathBuf) -> PathBuf {
    if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    }
}
