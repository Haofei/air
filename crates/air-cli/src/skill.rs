use crate::code_agent::{
    build_input, code_profile_explain, path_ref_to_input_string, run_code_agent, CodeOptions,
};
use crate::code_artifact::{path_content_identity, CodeRunSkill};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

const SKILL_MANIFEST: &str = "air-skill.yaml";
const SKILL_SCHEMA: &str = "air.skill.v1";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct AirSkillManifest {
    #[serde(default)]
    pub(crate) schema: Option<String>,
    pub(crate) id: String,
    #[serde(default)]
    pub(crate) version: Option<String>,
    #[serde(default)]
    pub(crate) description: Option<String>,
    #[serde(default)]
    pub(crate) source: Option<Value>,
    #[serde(default)]
    pub(crate) instructions: SkillInstructions,
    #[serde(default)]
    pub(crate) capabilities: SkillCapabilities,
    #[serde(default)]
    pub(crate) tools: SkillTools,
    pub(crate) workflow: SkillWorkflow,
    #[serde(default)]
    pub(crate) verification: Value,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct SkillInstructions {
    #[serde(default)]
    pub(crate) files: Vec<PathBuf>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct SkillCapabilities {
    #[serde(default)]
    pub(crate) allow: Vec<String>,
    #[serde(default)]
    pub(crate) deny: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct SkillTools {
    #[serde(default)]
    pub(crate) config: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SkillWorkflow {
    pub(crate) profile: PathBuf,
}

#[derive(Debug, Clone)]
pub(crate) struct ResolvedSkill {
    pub(crate) manifest_path: PathBuf,
    pub(crate) manifest_dir: PathBuf,
    pub(crate) manifest: AirSkillManifest,
}

pub(crate) struct SkillRunOptions {
    pub(crate) reference: String,
    pub(crate) task: String,
    pub(crate) profile_override: Option<PathBuf>,
    pub(crate) model_config: Option<PathBuf>,
    pub(crate) trace_out: Option<PathBuf>,
    pub(crate) trace_redact: bool,
    pub(crate) trace_raw: bool,
    pub(crate) log: bool,
    pub(crate) tool_config_override: Option<PathBuf>,
    pub(crate) artifact_out: Option<PathBuf>,
    pub(crate) replay_artifact: Option<PathBuf>,
    pub(crate) replay_from: Option<usize>,
}

#[derive(Debug, Serialize)]
struct SkillListItem {
    id: String,
    version: Option<String>,
    description: Option<String>,
    manifest: String,
    profile: String,
    tool_config: Option<String>,
    run: String,
    explain: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SkillAudit {
    pub(crate) schema: String,
    pub(crate) skill_id: String,
    pub(crate) risk: String,
    pub(crate) allowed_to_run: bool,
    pub(crate) findings: Vec<SkillAuditFinding>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SkillAuditFinding {
    pub(crate) severity: String,
    pub(crate) reason: String,
    pub(crate) file: String,
}

pub(crate) fn list_skills() -> Result<()> {
    let skills = discover_skills()?
        .into_iter()
        .map(|skill| skill.list_item())
        .collect::<Result<Vec<_>>>()?;
    print_json(&json!({ "skills": skills }))
}

pub(crate) fn validate_skill(reference: &str) -> Result<()> {
    let skill = resolve_skill(reference)?;
    let checks = validate_resolved_skill(&skill)?;
    print_json(&json!({
        "valid": true,
        "skill": skill.manifest.id,
        "manifest": path_ref(&skill.manifest_path),
        "checks": checks,
    }))
}

pub(crate) fn explain_skill(reference: &str, profile_override: Option<PathBuf>) -> Result<()> {
    let skill = resolve_skill(reference)?;
    validate_resolved_skill(&skill)?;
    let profile = profile_override.unwrap_or_else(|| skill.profile_path());
    let input = build_input("<task>".to_string());
    let mut explanation = code_profile_explain(&skill.manifest.id, &profile, &input)?;
    if let Some(object) = explanation.as_object_mut() {
        object.insert(
            "manifest".to_string(),
            Value::String(path_ref(&skill.manifest_path)),
        );
        object.insert(
            "description".to_string(),
            skill
                .manifest
                .description
                .clone()
                .map_or(Value::Null, Value::String),
        );
        object.insert(
            "instructions".to_string(),
            Value::Array(
                skill
                    .manifest
                    .instructions
                    .files
                    .iter()
                    .map(|path| Value::String(path_ref(&skill.manifest_dir.join(path))))
                    .collect(),
            ),
        );
    }
    print_json(&explanation)
}

pub(crate) fn compile_skill(reference: &str, output: Option<PathBuf>) -> Result<()> {
    let skill = resolve_skill(reference)?;
    validate_resolved_skill(&skill)?;
    let compiled = json!({
        "schema": "air.compiled_skill.v1",
        "skill": skill_metadata(&skill)?,
        "manifest": path_ref(&skill.manifest_path),
        "profile": path_ref(&skill.profile_path()),
        "tool_config": skill.tool_config_path().map(|path| path_ref(&path)),
        "instructions": skill.manifest.instructions.files.iter().map(|path| path_ref(&skill.manifest_dir.join(path))).collect::<Vec<_>>(),
        "capabilities": {
            "allow": skill.manifest.capabilities.allow,
            "deny": skill.manifest.capabilities.deny,
        },
        "verification": skill.manifest.verification,
    });
    if let Some(output) = output {
        if let Some(parent) = output.parent() {
            fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
        }
        fs::write(&output, serde_json::to_vec_pretty(&compiled)?)
            .with_context(|| format!("write {}", output.display()))?;
        print_json(&json!({ "compiled": path_ref(&output) }))
    } else {
        print_json(&compiled)
    }
}

pub(crate) fn run_skill(options: SkillRunOptions) -> Result<()> {
    let skill = resolve_skill(&options.reference)?;
    validate_resolved_skill(&skill)?;
    let metadata = skill_metadata(&skill)?;
    let task = skill_run_task(&skill.manifest.id, &options.task);
    let outputs = run_code_agent(CodeOptions {
        task,
        skill: Some(metadata.clone()),
        profile: Some(
            options
                .profile_override
                .unwrap_or_else(|| skill.profile_path()),
        ),
        model_config: options.model_config,
        trace_out: options.trace_out,
        trace_redact: options.trace_redact,
        trace_raw: options.trace_raw,
        log: options.log,
        tool_config: options
            .tool_config_override
            .or_else(|| skill.tool_config_path()),
        artifact_out: options.artifact_out,
        artifact_extra: BTreeMap::from([("skill".to_string(), json!(metadata))]),
        replay_artifact: options.replay_artifact,
        replay_from: options.replay_from,
    })?;
    print_json(&outputs)
}

fn skill_run_task(skill_id: &str, task: &str) -> String {
    if skill_id == "code-agent" {
        task.to_string()
    } else {
        format!("Use the `{skill_id}` skill before acting.\n\nTask: {task}")
    }
}

pub(crate) fn audit_skill(reference: &str, write: bool) -> Result<()> {
    let skill = resolve_skill(reference)?;
    let audit = audit_resolved_skill(&skill)?;
    if write {
        let path = skill.manifest_dir.join("audit.json");
        fs::write(&path, serde_json::to_vec_pretty(&audit)?)
            .with_context(|| format!("write {}", path.display()))?;
    }
    print_json(&json!(audit))
}

pub(crate) fn import_skill(source: &str, out: &Path) -> Result<()> {
    if out.exists() {
        bail!("skill import output already exists: {}", out.display());
    }
    let temp_dir = std::env::temp_dir().join(format!(
        "air-skill-import-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    fs::create_dir_all(&temp_dir).with_context(|| format!("create {}", temp_dir.display()))?;
    let imported_root = import_source_to_temp(source, &temp_dir)?;
    fs::create_dir_all(out.parent().unwrap_or_else(|| Path::new(".")))
        .with_context(|| format!("create parent for {}", out.display()))?;
    copy_dir_all(&imported_root, out)?;
    ensure_import_manifest(source, out)?;
    let resolved = resolve_skill(out.to_string_lossy().as_ref())?;
    let audit = audit_resolved_skill(&resolved)?;
    fs::write(out.join("audit.json"), serde_json::to_vec_pretty(&audit)?)
        .with_context(|| format!("write {}", out.join("audit.json").display()))?;
    fs::write(
        out.join("source.json"),
        serde_json::to_vec_pretty(&json!({
            "source": source,
            "imported_at_unix_ms": SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis(),
            "manifest": path_ref(&resolved.manifest_path),
            "sha256": directory_identity(out)?,
        }))?,
    )
    .with_context(|| format!("write {}", out.join("source.json").display()))?;
    let _ = fs::remove_dir_all(temp_dir);
    print_json(&json!({
        "imported": path_ref(out),
        "skill": resolved.manifest.id,
        "audit_risk": audit.risk,
    }))
}

pub(crate) fn resolve_skill(reference: &str) -> Result<ResolvedSkill> {
    let path = PathBuf::from(reference);
    let manifest_path = if path.exists() {
        manifest_path_for_existing_path(&path)?
    } else {
        find_skill_manifest_by_id(reference)
            .with_context(|| format!("unknown AIR skill `{reference}`; run `air skill list`"))?
    };
    read_skill_manifest(&manifest_path)
}

pub(crate) fn resolve_skill_run_metadata(reference: &str) -> Result<CodeRunSkill> {
    let skill = resolve_skill(reference)?;
    skill_metadata(&skill)
}

fn discover_skills() -> Result<Vec<ResolvedSkill>> {
    let mut skills = Vec::new();
    for root in skill_roots() {
        if !root.exists() {
            continue;
        }
        collect_skill_manifests_from_dir(&root, &mut skills)?;
        collect_skill_manifests_from_dir(&root.join("vendor"), &mut skills)?;
    }
    let mut seen = BTreeSet::new();
    skills.retain(|skill| seen.insert(skill.manifest.id.clone()));
    skills.sort_by(|left, right| left.manifest.id.cmp(&right.manifest.id));
    Ok(skills)
}

fn collect_skill_manifests_from_dir(root: &Path, skills: &mut Vec<ResolvedSkill>) -> Result<()> {
    if !root.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(root).with_context(|| format!("read {}", root.display()))? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let manifest = entry.path().join(SKILL_MANIFEST);
        if manifest.exists() {
            skills.push(read_skill_manifest(&manifest)?);
        }
    }
    Ok(())
}

fn find_skill_manifest_by_id(skill_id: &str) -> Result<PathBuf> {
    for root in skill_roots() {
        for candidate in [
            root.join(skill_id).join(SKILL_MANIFEST),
            root.join("vendor").join(skill_id).join(SKILL_MANIFEST),
        ] {
            if candidate.exists() {
                return Ok(candidate);
            }
        }
    }
    bail!("skill manifest not found")
}

fn skill_roots() -> Vec<PathBuf> {
    vec![
        PathBuf::from("skills"),
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("skills"),
    ]
}

fn read_skill_manifest(path: &Path) -> Result<ResolvedSkill> {
    let manifest_content =
        fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let manifest: AirSkillManifest = serde_yaml::from_str(&manifest_content)
        .with_context(|| format!("parse {}", path.display()))?;
    let manifest_dir = path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();
    Ok(ResolvedSkill {
        manifest_path: path.to_path_buf(),
        manifest_dir,
        manifest,
    })
}

fn manifest_path_for_existing_path(path: &Path) -> Result<PathBuf> {
    if path.is_dir() {
        let manifest = path.join(SKILL_MANIFEST);
        if manifest.exists() {
            return Ok(manifest);
        }
        bail!(
            "skill directory missing {SKILL_MANIFEST}: {}",
            path.display()
        );
    }
    if path.file_name().and_then(|name| name.to_str()) == Some(SKILL_MANIFEST) {
        return Ok(path.to_path_buf());
    }
    bail!(
        "skill path must be a directory or {SKILL_MANIFEST}: {}",
        path.display()
    )
}

fn validate_resolved_skill(skill: &ResolvedSkill) -> Result<Value> {
    if skill.manifest.schema.as_deref().unwrap_or(SKILL_SCHEMA) != SKILL_SCHEMA {
        bail!(
            "unsupported skill schema `{}`",
            skill.manifest.schema.as_deref().unwrap_or_default()
        );
    }
    if skill.manifest.id.trim().is_empty() {
        bail!("skill id must not be empty");
    }
    let profile = skill.profile_path();
    if !profile.exists() {
        bail!(
            "skill workflow profile does not exist: {}",
            profile.display()
        );
    }
    if let Some(tool_config) = skill.tool_config_path() {
        if !tool_config.exists() {
            bail!(
                "skill tool config does not exist: {}",
                tool_config.display()
            );
        }
    }
    for file in &skill.manifest.instructions.files {
        let path = skill.manifest_dir.join(file);
        if !path.exists() {
            bail!("skill instruction file does not exist: {}", path.display());
        }
    }
    let allow = skill
        .manifest
        .capabilities
        .allow
        .iter()
        .collect::<BTreeSet<_>>();
    let conflicts = skill
        .manifest
        .capabilities
        .deny
        .iter()
        .filter(|capability| allow.contains(*capability))
        .cloned()
        .collect::<Vec<_>>();
    if !conflicts.is_empty() {
        bail!("skill capability appears in both allow and deny: {conflicts:?}");
    }
    Ok(json!({
        "profile_exists": true,
        "tool_config_exists": skill.tool_config_path().is_some(),
        "instruction_files": skill.manifest.instructions.files.len(),
        "capability_allow": skill.manifest.capabilities.allow,
        "capability_deny": skill.manifest.capabilities.deny,
    }))
}

fn audit_resolved_skill(skill: &ResolvedSkill) -> Result<SkillAudit> {
    let mut findings = Vec::new();
    for path in skill_files(&skill.manifest_dir)? {
        let content = match fs::read_to_string(&path) {
            Ok(content) => content,
            Err(_) => continue,
        };
        let relative = path_ref(path.strip_prefix(&skill.manifest_dir).unwrap_or(&path));
        let lower = content.to_ascii_lowercase();
        for (severity, reason, needles) in audit_patterns() {
            if needles.iter().any(|needle| lower.contains(needle)) {
                findings.push(SkillAuditFinding {
                    severity: severity.to_string(),
                    reason: reason.to_string(),
                    file: relative.clone(),
                });
            }
        }
        if is_executable_script_path(&path) {
            findings.push(SkillAuditFinding {
                severity: "medium".to_string(),
                reason: "skill contains executable script content".to_string(),
                file: relative,
            });
        }
    }
    for capability in &skill.manifest.capabilities.allow {
        if matches!(
            capability.as_str(),
            "shell" | "shell.unrestricted" | "network" | "secrets.read"
        ) {
            findings.push(SkillAuditFinding {
                severity: "high".to_string(),
                reason: format!("skill requests high-risk capability `{capability}`"),
                file: path_ref(&skill.manifest_path),
            });
        }
    }
    let risk = audit_risk(&findings);
    Ok(SkillAudit {
        schema: "air.skill_audit.v1".to_string(),
        skill_id: skill.manifest.id.clone(),
        allowed_to_run: !matches!(risk.as_str(), "critical" | "high"),
        risk,
        findings,
    })
}

fn audit_patterns() -> Vec<(&'static str, &'static str, Vec<&'static str>)> {
    vec![
        (
            "critical",
            "remote shell execution pattern",
            vec![
                "curl | sh",
                "curl -fs",
                "wget | sh",
                "iex(",
                "powershell -enc",
            ],
        ),
        (
            "critical",
            "secret or wallet path access pattern",
            vec![
                "~/.ssh",
                ".aws/credentials",
                "login keychain",
                "metamask",
                "wallet.dat",
            ],
        ),
        (
            "high",
            "environment or credential exfiltration pattern",
            vec![
                "process.env",
                "os.environ",
                "printenv",
                "cat .env",
                "env |",
                "dotenv",
            ],
        ),
        (
            "high",
            "dynamic command execution pattern",
            vec!["bash -c", "sh -c", "python -c", "node -e", "eval "],
        ),
    ]
}

fn audit_risk(findings: &[SkillAuditFinding]) -> String {
    if findings
        .iter()
        .any(|finding| finding.severity == "critical")
    {
        "critical".to_string()
    } else if findings.iter().any(|finding| finding.severity == "high") {
        "high".to_string()
    } else if findings.iter().any(|finding| finding.severity == "medium") {
        "medium".to_string()
    } else {
        "low".to_string()
    }
}

fn skill_files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    collect_skill_files(root, root, &mut files)?;
    Ok(files)
}

fn collect_skill_files(root: &Path, dir: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(dir).with_context(|| format!("read {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        let relative = path.strip_prefix(root).unwrap_or(&path);
        if relative.starts_with(".git")
            || relative.starts_with("target")
            || relative.starts_with("node_modules")
        {
            continue;
        }
        if entry.file_type()?.is_dir() {
            collect_skill_files(root, &path, files)?;
        } else if entry.file_type()?.is_file() {
            files.push(path);
        }
    }
    Ok(())
}

fn is_executable_script_path(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|extension| extension.to_str()),
        Some("sh" | "bash" | "zsh" | "ps1" | "py" | "js" | "ts")
    )
}

fn import_source_to_temp(source: &str, temp_dir: &Path) -> Result<PathBuf> {
    let source_path = PathBuf::from(source);
    if source_path.exists() {
        if source_path.is_dir() {
            let destination = temp_dir.join("source");
            copy_dir_all(&source_path, &destination)?;
            return Ok(destination);
        }
        if source_path
            .extension()
            .and_then(|extension| extension.to_str())
            == Some("zip")
        {
            return unzip_to_temp(&source_path, temp_dir);
        }
        bail!("skill import source must be a directory, git URL, or zip: {source}");
    }
    if let Some(github_tree) = parse_github_tree_url(source) {
        let destination = temp_dir.join("repo");
        run_status(
            Command::new("git")
                .args(["clone", "--depth", "1", "--branch"])
                .arg(&github_tree.branch)
                .arg(&github_tree.repo_url)
                .arg(&destination),
            "git clone skill source",
        )?;
        let skill_dir = destination.join(&github_tree.subpath);
        if !skill_dir.exists() {
            bail!(
                "GitHub tree skill path does not exist after clone: {}",
                github_tree.subpath.display()
            );
        }
        return Ok(skill_dir);
    }
    if source.ends_with(".git") || source.contains("github.com") {
        let destination = temp_dir.join("repo");
        run_status(
            Command::new("git")
                .args(["clone", "--depth", "1", source])
                .arg(&destination),
            "git clone skill source",
        )?;
        return Ok(destination);
    }
    if source.starts_with("http://") || source.starts_with("https://") {
        let zip_path = temp_dir.join("source.zip");
        run_status(
            Command::new("curl")
                .args(["-L", "--fail", source, "-o"])
                .arg(&zip_path),
            "download skill zip",
        )?;
        return unzip_to_temp(&zip_path, temp_dir);
    }
    bail!("skill import source does not exist: {source}")
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct GitHubTreeSource {
    repo_url: String,
    branch: String,
    subpath: PathBuf,
}

fn parse_github_tree_url(source: &str) -> Option<GitHubTreeSource> {
    let clean = source.split(['?', '#']).next()?.trim_end_matches('/');
    let path = clean
        .strip_prefix("https://github.com/")
        .or_else(|| clean.strip_prefix("http://github.com/"))?;
    let segments = path.split('/').collect::<Vec<_>>();
    if segments.len() < 5 || segments[2] != "tree" {
        return None;
    }
    let owner = segments[0];
    let repo = segments[1].trim_end_matches(".git");
    let branch = segments[3].to_string();
    let mut subpath = PathBuf::new();
    for segment in &segments[4..] {
        subpath.push(segment);
    }
    Some(GitHubTreeSource {
        repo_url: format!("https://github.com/{owner}/{repo}.git"),
        branch,
        subpath,
    })
}

fn unzip_to_temp(zip_path: &Path, temp_dir: &Path) -> Result<PathBuf> {
    let destination = temp_dir.join("unzipped");
    fs::create_dir_all(&destination)
        .with_context(|| format!("create {}", destination.display()))?;
    run_status(
        Command::new("unzip")
            .args(["-q"])
            .arg(zip_path)
            .args(["-d"])
            .arg(&destination),
        "unzip skill source",
    )?;
    single_child_dir_or_self(&destination)
}

fn single_child_dir_or_self(path: &Path) -> Result<PathBuf> {
    let children = fs::read_dir(path)?
        .filter_map(Result::ok)
        .collect::<Vec<_>>();
    if children.len() == 1 && children[0].file_type()?.is_dir() {
        Ok(children[0].path())
    } else {
        Ok(path.to_path_buf())
    }
}

fn ensure_import_manifest(source: &str, out: &Path) -> Result<()> {
    let manifest_path = out.join(SKILL_MANIFEST);
    if manifest_path.exists() {
        return Ok(());
    }
    let id = out
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("imported-skill")
        .to_string();
    let description = format!("Imported AIR skill wrapper for {source}");
    let default_profile = resolve_skill("code-agent")
        .map(|skill| {
            let profile = skill.profile_path();
            relative_path_from(out, &profile).unwrap_or_else(|| absolutize_path(&profile))
        })
        .unwrap_or_else(|_| {
            std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join("skills/code-agent/edit.air-profile.yaml")
        });
    let manifest = AirSkillManifest {
        schema: Some(SKILL_SCHEMA.to_string()),
        id,
        version: Some("0.1.0".to_string()),
        description: Some(description),
        source: Some(json!({
            "type": "imported",
            "original": source,
        })),
        instructions: SkillInstructions {
            files: find_instruction_files(out)?,
        },
        capabilities: SkillCapabilities {
            allow: vec!["file.read".to_string(), "code.read".to_string()],
            deny: vec![
                "file.write".to_string(),
                "shell.unrestricted".to_string(),
                "network".to_string(),
                "secrets.read".to_string(),
            ],
        },
        tools: SkillTools { config: None },
        workflow: SkillWorkflow {
            profile: default_profile,
        },
        verification: json!({
            "mode": "instruction_only",
            "scripts": "disabled",
        }),
    };
    fs::write(&manifest_path, serde_yaml::to_string(&manifest)?)
        .with_context(|| format!("write {}", manifest_path.display()))
}

fn find_instruction_files(root: &Path) -> Result<Vec<PathBuf>> {
    for name in ["SKILL.md", "README.md"] {
        if root.join(name).exists() {
            return Ok(vec![PathBuf::from(name)]);
        }
    }
    Ok(Vec::new())
}

fn copy_dir_all(source: &Path, destination: &Path) -> Result<()> {
    fs::create_dir_all(destination).with_context(|| format!("create {}", destination.display()))?;
    for entry in fs::read_dir(source).with_context(|| format!("read {}", source.display()))? {
        let entry = entry?;
        if entry.file_name() == ".git" {
            continue;
        }
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_all(&source_path, &destination_path)?;
        } else {
            fs::copy(&source_path, &destination_path).with_context(|| {
                format!(
                    "copy {} to {}",
                    source_path.display(),
                    destination_path.display()
                )
            })?;
        }
    }
    Ok(())
}

fn run_status(command: &mut Command, label: &str) -> Result<()> {
    let status = command.status().with_context(|| format!("run {label}"))?;
    if !status.success() {
        bail!("{label} failed with status {status}");
    }
    Ok(())
}

fn directory_identity(root: &Path) -> Result<String> {
    let mut items = Vec::new();
    for file in skill_files(root)? {
        let relative = path_ref(file.strip_prefix(root).unwrap_or(&file));
        let identity = path_content_identity(&file)?;
        items.push(json!({ "path": relative, "identity": identity }));
    }
    items.sort_by(|left, right| left["path"].as_str().cmp(&right["path"].as_str()));
    let encoded = serde_json::to_vec(&items)?;
    Ok(format!(
        "sha256:{}",
        crate::code_artifact::sha256_hex(&encoded)
    ))
}

fn skill_metadata(skill: &ResolvedSkill) -> Result<CodeRunSkill> {
    let audit_risk = read_audit_risk(&skill.manifest_dir.join("audit.json"));
    Ok(CodeRunSkill {
        id: skill.manifest.id.clone(),
        version: skill.manifest.version.clone(),
        source: skill.source_label(),
        manifest: Some(path_ref(&skill.manifest_path)),
        manifest_sha256: Some(path_content_identity(&skill.manifest_path)?),
        audit_risk,
    })
}

fn read_audit_risk(path: &Path) -> Option<String> {
    let content = fs::read(path).ok()?;
    let audit: SkillAudit = serde_json::from_slice(&content).ok()?;
    Some(audit.risk)
}

impl ResolvedSkill {
    fn profile_path(&self) -> PathBuf {
        resolve_skill_path(&self.manifest_dir, &self.manifest.workflow.profile)
    }

    fn tool_config_path(&self) -> Option<PathBuf> {
        self.manifest
            .tools
            .config
            .as_ref()
            .map(|path| resolve_skill_path(&self.manifest_dir, path))
    }

    fn source_label(&self) -> Option<String> {
        self.manifest.source.as_ref().and_then(|source| {
            source
                .get("type")
                .and_then(Value::as_str)
                .or_else(|| source.as_str())
                .map(str::to_string)
        })
    }

    fn list_item(&self) -> Result<SkillListItem> {
        Ok(SkillListItem {
            id: self.manifest.id.clone(),
            version: self.manifest.version.clone(),
            description: self.manifest.description.clone(),
            manifest: path_ref(&self.manifest_path),
            profile: path_ref(&self.profile_path()),
            tool_config: self.tool_config_path().map(|path| path_ref(&path)),
            run: format!("air skill run {} \"<task>\"", self.manifest.id),
            explain: format!("air skill explain {}", self.manifest.id),
        })
    }
}

fn resolve_skill_path(base: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    }
}

fn absolutize_path(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path)
    }
}

fn relative_path_from(base_dir: &Path, target: &Path) -> Option<PathBuf> {
    let base = absolutize_path(base_dir).canonicalize().ok()?;
    let target = absolutize_path(target).canonicalize().ok()?;
    let base_components = path_components(&base)?;
    let target_components = path_components(&target)?;
    let common = base_components
        .iter()
        .zip(target_components.iter())
        .take_while(|(left, right)| left == right)
        .count();
    if common == 0 {
        return None;
    }
    let mut relative = PathBuf::new();
    for _ in common..base_components.len() {
        relative.push("..");
    }
    for component in target_components.iter().skip(common) {
        relative.push(component);
    }
    if relative.as_os_str().is_empty() {
        Some(PathBuf::from("."))
    } else {
        Some(relative)
    }
}

fn path_components(path: &Path) -> Option<Vec<OsString>> {
    let mut components = Vec::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => components.push(prefix.as_os_str().to_os_string()),
            Component::RootDir => components.push(OsString::from(std::path::MAIN_SEPARATOR_STR)),
            Component::Normal(part) => components.push(part.to_os_string()),
            Component::CurDir => {}
            Component::ParentDir => return None,
        }
    }
    Some(components)
}

fn path_ref(path: &Path) -> String {
    path_ref_to_input_string(path)
}

fn print_json(value: &Value) -> Result<()> {
    serde_json::to_writer_pretty(std::io::stdout(), value)?;
    println!();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_builtin_code_agent_skill_manifest() {
        let skill = resolve_skill("code-agent").unwrap();
        assert_eq!(skill.manifest.id, "code-agent");
        assert!(skill.profile_path().ends_with("edit.air-profile.yaml"));
        validate_resolved_skill(&skill).unwrap();
    }

    #[test]
    fn audits_remote_shell_patterns() {
        let temp = std::env::temp_dir().join(format!(
            "air-skill-audit-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&temp).unwrap();
        fs::write(temp.join("SKILL.md"), "Run curl | sh to install").unwrap();
        fs::write(
            temp.join(SKILL_MANIFEST),
            r#"
schema: air.skill.v1
id: unsafe
workflow:
  profile: profile.air-profile.yaml
instructions:
  files: [SKILL.md]
"#,
        )
        .unwrap();
        fs::write(
            temp.join("profile.air-profile.yaml"),
            "plan: plan\nstore: store\n",
        )
        .unwrap();
        let skill = read_skill_manifest(&temp.join(SKILL_MANIFEST)).unwrap();
        let audit = audit_resolved_skill(&skill).unwrap();
        assert_eq!(audit.risk, "critical");
        let _ = fs::remove_dir_all(temp);
    }

    #[test]
    fn external_skill_run_task_prompts_model_to_load_skill() {
        assert_eq!(skill_run_task("code-agent", "fix bug"), "fix bug");
        assert!(skill_run_task("tdd-workflow", "fix bug").contains("Use the `tdd-workflow` skill"));
    }

    #[test]
    fn parses_github_tree_skill_url() {
        let parsed = parse_github_tree_url(
            "https://github.com/affaan-m/everything-claude-code/tree/main/skills/tdd-workflow",
        )
        .unwrap();
        assert_eq!(
            parsed.repo_url,
            "https://github.com/affaan-m/everything-claude-code.git"
        );
        assert_eq!(parsed.branch, "main");
        assert_eq!(parsed.subpath, PathBuf::from("skills/tdd-workflow"));
    }
}
