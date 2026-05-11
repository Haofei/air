use crate::models::ModelProviderChoice;
use crate::planner::module_base_dir_for_store_path;
use crate::profile::{read_json_object, read_run_plan_profile, resolve_profile_path};
use crate::tools::ToolProviderChoice;
use air_runtime::{
    read_trace_jsonl, replay_outputs, system_return_event, write_trace_jsonl_with_options,
    TraceEvent, TraceStatus, TraceWriteOptions,
};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

pub(crate) struct RunPlanOptions {
    pub(crate) plan: Option<PathBuf>,
    pub(crate) profile: Option<PathBuf>,
    pub(crate) store: Option<PathBuf>,
    pub(crate) input: Option<PathBuf>,
    pub(crate) input_values: Option<serde_json::Map<String, Value>>,
    pub(crate) model_config: Option<PathBuf>,
    pub(crate) trace_out: Option<PathBuf>,
    pub(crate) trace_redact: bool,
    pub(crate) trace_raw: bool,
    pub(crate) state_out: Option<PathBuf>,
    pub(crate) checkpoint_out: Option<PathBuf>,
    pub(crate) jit_cache: Option<PathBuf>,
    pub(crate) parallel: bool,
    pub(crate) log: bool,
    pub(crate) example_tools: bool,
    pub(crate) tool_config: Option<PathBuf>,
}

pub(crate) struct ResumePlanOptions {
    pub(crate) plan: Option<PathBuf>,
    pub(crate) profile: Option<PathBuf>,
    pub(crate) store: Option<PathBuf>,
    pub(crate) input: Option<PathBuf>,
    pub(crate) state: PathBuf,
    pub(crate) overrides: Vec<String>,
    pub(crate) model_config: Option<PathBuf>,
    pub(crate) trace_out: Option<PathBuf>,
    pub(crate) trace_redact: bool,
    pub(crate) trace_raw: bool,
    pub(crate) state_out: Option<PathBuf>,
    pub(crate) checkpoint_out: Option<PathBuf>,
    pub(crate) log: bool,
    pub(crate) example_tools: bool,
    pub(crate) tool_config: Option<PathBuf>,
}

pub(crate) struct RunPlanExecutionOptions {
    pub(crate) model_config: Option<PathBuf>,
    pub(crate) trace_out: Option<PathBuf>,
    pub(crate) trace_redact: bool,
    pub(crate) state_out: Option<PathBuf>,
    pub(crate) checkpoint_out: Option<PathBuf>,
    pub(crate) jit_cache: Option<PathBuf>,
    pub(crate) parallel: bool,
    pub(crate) log: bool,
    pub(crate) example_tools: bool,
    pub(crate) tool_config: Option<PathBuf>,
}

pub(crate) struct ReplayOptions {
    pub(crate) trace: PathBuf,
    pub(crate) specialize_run_plan: bool,
    pub(crate) store: Option<PathBuf>,
    pub(crate) output: Option<PathBuf>,
    pub(crate) identity_out: Option<PathBuf>,
    pub(crate) stats: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct PlanStateFile {
    pub(crate) status: air_linker::RunStatus,
    pub(crate) outputs: Value,
    pub(crate) module_outputs: BTreeMap<String, Value>,
}

fn effective_trace_redact(
    cli_trace_redact: bool,
    cli_trace_raw: bool,
    profile: Option<&(PathBuf, crate::profile::RunPlanProfile)>,
) -> bool {
    if cli_trace_redact {
        return true;
    }
    if cli_trace_raw {
        return false;
    }
    if let Some((_, profile)) = profile {
        if profile.trace_raw.unwrap_or(false) {
            return false;
        }
        return profile.trace_redact.unwrap_or(true);
    }
    true
}

pub(crate) fn run_plan(options: RunPlanOptions) -> Result<()> {
    let outputs = run_plan_capture(options)?;
    println!("{}", serde_json::to_string_pretty(&outputs)?);
    Ok(())
}

pub(crate) fn run_plan_capture(options: RunPlanOptions) -> Result<Value> {
    let RunPlanOptions {
        plan,
        profile,
        store,
        input,
        input_values,
        model_config,
        trace_out,
        trace_redact,
        trace_raw,
        state_out,
        checkpoint_out,
        jit_cache,
        parallel,
        log,
        example_tools,
        tool_config,
    } = options;

    let profile = match profile {
        Some(path) => Some((path.clone(), read_run_plan_profile(&path)?)),
        None => None,
    };

    let plan = plan
        .or_else(|| {
            profile
                .as_ref()
                .map(|(path, profile)| resolve_profile_path(path, &profile.plan))
        })
        .ok_or_else(|| anyhow::anyhow!("run-plan requires a plan path or --profile"))?;
    let store = store
        .or_else(|| {
            profile
                .as_ref()
                .map(|(path, profile)| resolve_profile_path(path, &profile.store))
        })
        .ok_or_else(|| anyhow::anyhow!("run-plan requires --store or --profile"))?;
    let inputs = if let Some(input_values) = input_values {
        input_values
    } else if let Some(input) = input {
        read_json_object(input, "--input")?
    } else if let Some((path, profile)) = &profile {
        if let Some(input_file) = &profile.input_file {
            read_json_object(resolve_profile_path(path, input_file), "profile.input_file")?
        } else if let Some(inputs) = &profile.inputs {
            serde_json::Map::from_iter(inputs.clone())
        } else {
            anyhow::bail!("run-plan requires --input, profile.input_file, or profile.inputs");
        }
    } else {
        anyhow::bail!("run-plan requires --input or --profile");
    };
    let model_config = model_config.or_else(|| {
        profile.as_ref().and_then(|(path, profile)| {
            profile
                .model_config
                .as_ref()
                .map(|value| resolve_profile_path(path, value))
        })
    });
    let tool_config = tool_config.or_else(|| {
        profile.as_ref().and_then(|(path, profile)| {
            profile
                .tool_config
                .as_ref()
                .map(|value| resolve_profile_path(path, value))
        })
    });
    let trace_out = trace_out.or_else(|| {
        profile.as_ref().and_then(|(path, profile)| {
            profile
                .trace_out
                .as_ref()
                .map(|value| resolve_profile_path(path, value))
        })
    });
    let state_out = state_out.or_else(|| {
        profile.as_ref().and_then(|(path, profile)| {
            profile
                .state_out
                .as_ref()
                .map(|value| resolve_profile_path(path, value))
        })
    });
    let checkpoint_out = checkpoint_out.or_else(|| {
        profile.as_ref().and_then(|(path, profile)| {
            profile
                .checkpoint_out
                .as_ref()
                .map(|value| resolve_profile_path(path, value))
        })
    });
    let jit_cache = jit_cache.or_else(|| {
        profile.as_ref().and_then(|(path, profile)| {
            profile
                .jit_cache
                .as_ref()
                .map(|value| resolve_profile_path(path, value))
        })
    });
    let parallel = parallel
        || profile
            .as_ref()
            .and_then(|(_, profile)| profile.parallel)
            .unwrap_or(false);
    let log = log
        || profile
            .as_ref()
            .and_then(|(_, profile)| profile.log)
            .unwrap_or(false);
    let trace_redact = effective_trace_redact(trace_redact, trace_raw, profile.as_ref());
    let example_tools = example_tools
        || profile
            .as_ref()
            .and_then(|(_, profile)| profile.example_tools)
            .unwrap_or(false);

    run_plan_with_inputs_capture(
        plan,
        store,
        inputs,
        RunPlanExecutionOptions {
            model_config,
            trace_out,
            trace_redact,
            state_out,
            checkpoint_out,
            jit_cache,
            parallel,
            log,
            example_tools,
            tool_config,
        },
    )
}

#[cfg(test)]
pub(crate) fn run_plan_with_inputs(
    plan: PathBuf,
    store: PathBuf,
    inputs: serde_json::Map<String, Value>,
    options: RunPlanExecutionOptions,
) -> Result<()> {
    let outputs = run_plan_with_inputs_capture(plan, store, inputs, options)?;
    println!("{}", serde_json::to_string_pretty(&outputs)?);
    Ok(())
}

pub(crate) fn run_plan_with_inputs_capture(
    plan: PathBuf,
    store: PathBuf,
    inputs: serde_json::Map<String, Value>,
    options: RunPlanExecutionOptions,
) -> Result<Value> {
    let RunPlanExecutionOptions {
        model_config,
        trace_out,
        trace_redact,
        state_out,
        checkpoint_out,
        jit_cache,
        parallel,
        log,
        example_tools,
        tool_config,
    } = options;

    let original_plan = air_linker::parse_run_plan_file(plan)?;
    let store_path = store;
    let store = air_linker::parse_module_store_file(&store_path)?;
    let base_dir = module_base_dir_for_store_path(&store, &store_path);
    let (plan, jit_context) = if let Some(jit_cache) = jit_cache.as_ref() {
        select_jit_cached_run_plan(original_plan, &store, &base_dir, &inputs, jit_cache, log)?
    } else {
        (original_plan, None)
    };
    let report = air_linker::validate_run_plan(&plan, &store, &base_dir);
    if !report.is_success() {
        for diagnostic in &report.diagnostics {
            eprintln!("error[{}]: {}", diagnostic.code, diagnostic.message);
        }
        std::process::exit(1);
    }

    let observe = log || trace_out.is_some();
    let mut observed_trace = Vec::new();
    let tools = ToolProviderChoice::from_config(tool_config, example_tools)?;
    let result = if parallel {
        if let Some(model_config) = model_config {
            let models = ModelProviderChoice::from_config_file(model_config)?;
            run_plan_parallel(
                &plan,
                &store,
                base_dir.clone(),
                inputs,
                tools,
                models,
                observe,
                log,
                trace_out.as_ref(),
                trace_redact,
                checkpoint_out.as_ref(),
                &mut observed_trace,
            )?
        } else {
            run_plan_parallel(
                &plan,
                &store,
                base_dir.clone(),
                inputs,
                tools,
                ModelProviderChoice::echo(),
                observe,
                log,
                trace_out.as_ref(),
                trace_redact,
                checkpoint_out.as_ref(),
                &mut observed_trace,
            )?
        }
    } else if let Some(model_config) = model_config {
        let models = ModelProviderChoice::from_config_file(model_config)?;
        if observe {
            let result = air_linker::run_run_plan_with_observer_and_checkpoint(
                &plan,
                &store,
                base_dir.clone(),
                inputs,
                tools,
                models,
                |event| {
                    observe_event_with_trace_file(
                        event,
                        log,
                        &mut observed_trace,
                        trace_out.as_ref(),
                        trace_redact,
                    )
                },
                |checkpoint| write_checkpoint_state(checkpoint_out.as_ref(), checkpoint),
            );
            if result.is_err() {
                write_partial_trace(trace_out.as_ref(), &observed_trace, trace_redact)?;
            }
            result?
        } else {
            air_linker::run_run_plan_with_observer_and_checkpoint(
                &plan,
                &store,
                base_dir.clone(),
                inputs,
                tools,
                models,
                |_| {},
                |checkpoint| write_checkpoint_state(checkpoint_out.as_ref(), checkpoint),
            )?
        }
    } else if observe {
        let result = air_linker::run_run_plan_with_observer_and_checkpoint(
            &plan,
            &store,
            base_dir.clone(),
            inputs,
            tools,
            ModelProviderChoice::echo(),
            |event| {
                observe_event_with_trace_file(
                    event,
                    log,
                    &mut observed_trace,
                    trace_out.as_ref(),
                    trace_redact,
                )
            },
            |checkpoint| write_checkpoint_state(checkpoint_out.as_ref(), checkpoint),
        );
        if result.is_err() {
            write_partial_trace(trace_out.as_ref(), &observed_trace, trace_redact)?;
        }
        result?
    } else {
        air_linker::run_run_plan_with_observer_and_checkpoint(
            &plan,
            &store,
            base_dir.clone(),
            inputs,
            tools,
            ModelProviderChoice::echo(),
            |_| {},
            |checkpoint| write_checkpoint_state(checkpoint_out.as_ref(), checkpoint),
        )?
    };
    if let Some(trace_out) = trace_out {
        let mut trace = result.trace.clone();
        trace.push(system_return_event(result.outputs.clone()));
        write_trace(trace_out, &trace, trace_redact)?;
    }
    if let Some(state_out) = state_out {
        write_plan_state(state_out, &result)?;
    }
    if let Some(jit_context) = jit_context.as_ref() {
        if !jit_context.hit {
            write_jit_cache(jit_context, &result.trace, &store, &base_dir, log)?;
        }
    }
    Ok(Value::Object(result.outputs))
}

#[derive(Debug, Clone)]
struct JitCacheContext {
    dir: PathBuf,
    key: String,
    hit: bool,
}

fn select_jit_cached_run_plan(
    original_plan: air_linker::RunPlan,
    store: &air_linker::ModuleStore,
    base_dir: &std::path::Path,
    inputs: &serde_json::Map<String, Value>,
    cache_dir: &std::path::Path,
    log: bool,
) -> Result<(air_linker::RunPlan, Option<JitCacheContext>)> {
    fs::create_dir_all(cache_dir)?;
    let key = jit_cache_key(&original_plan, store, base_dir, inputs)?;
    let context = JitCacheContext {
        dir: cache_dir.to_path_buf(),
        key,
        hit: false,
    };
    let plan_path = jit_cache_plan_path(&context);
    if plan_path.exists() {
        match air_linker::parse_run_plan_file(&plan_path) {
            Ok(cached_plan) => {
                let report = air_linker::validate_run_plan(&cached_plan, store, base_dir);
                if report.is_success() {
                    if log {
                        eprintln!(
                            "[air-jit] cache hit key={} plan={}",
                            context.key,
                            plan_path.display()
                        );
                    }
                    return Ok((
                        cached_plan,
                        Some(JitCacheContext {
                            hit: true,
                            ..context
                        }),
                    ));
                }
                if log {
                    eprintln!(
                        "[air-jit] ignoring invalid cache key={} diagnostics={:?}",
                        context.key, report.diagnostics
                    );
                }
            }
            Err(error) => {
                if log {
                    eprintln!(
                        "[air-jit] ignoring unreadable cache key={} error={error}",
                        context.key
                    );
                }
            }
        }
    } else if log {
        eprintln!("[air-jit] cache miss key={}", context.key);
    }

    Ok((original_plan, Some(context)))
}

fn write_jit_cache(
    context: &JitCacheContext,
    trace: &[TraceEvent],
    store: &air_linker::ModuleStore,
    base_dir: &std::path::Path,
    log: bool,
) -> Result<()> {
    let specialization = air_linker::specialize_run_plan_trace(trace, store, base_dir)?;
    let plan_path = jit_cache_plan_path(context);
    let identity_path = context.dir.join(format!("{}.identity.json", context.key));
    let meta_path = context.dir.join(format!("{}.meta.json", context.key));
    fs::create_dir_all(&context.dir)?;
    fs::write(&plan_path, serde_yaml::to_string(&specialization.plan)?)?;
    fs::write(
        &identity_path,
        serde_json::to_string_pretty(&specialization.cache_identity)?,
    )?;
    fs::write(
        &meta_path,
        serde_json::to_string_pretty(&json!({
            "kind": "air.jit_cache_entry.v1",
            "key": context.key,
            "plan": specialization.plan.plan,
            "identity_path": identity_path.file_name().and_then(|name| name.to_str()).unwrap_or_default(),
        }))?,
    )?;
    if log {
        eprintln!(
            "[air-jit] wrote cache key={} plan={}",
            context.key,
            plan_path.display()
        );
    }
    Ok(())
}

fn jit_cache_plan_path(context: &JitCacheContext) -> PathBuf {
    context.dir.join(format!("{}.air-plan.yaml", context.key))
}

fn jit_cache_key(
    plan: &air_linker::RunPlan,
    store: &air_linker::ModuleStore,
    base_dir: &std::path::Path,
    inputs: &serde_json::Map<String, Value>,
) -> Result<String> {
    let modules = store
        .modules
        .iter()
        .map(|(id, module_ref)| {
            let path = air_linker::resolve_module_path(base_dir, id, &module_ref.path)?;
            let content = fs::read_to_string(&path)?;
            Ok((
                id.clone(),
                json!({
                    "ref": module_ref,
                    "content": content,
                }),
            ))
        })
        .collect::<Result<BTreeMap<_, _>>>()?;
    let identity = json!({
        "kind": "air.jit_cache_key.v1",
        "plan": plan,
        "store": {
            "metadata": &store.store,
            "modules": modules,
            "recipes": &store.recipes,
        },
        "inputs": inputs,
    });
    let encoded = serde_json::to_string(&identity)?;
    let digest = ring::digest::digest(&ring::digest::SHA256, encoded.as_bytes());
    Ok(digest
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

#[allow(clippy::too_many_arguments)]
fn run_plan_parallel(
    plan: &air_linker::RunPlan,
    store: &air_linker::ModuleStore,
    base_dir: PathBuf,
    inputs: serde_json::Map<String, Value>,
    tools: ToolProviderChoice,
    models: ModelProviderChoice,
    observe: bool,
    log: bool,
    trace_out: Option<&PathBuf>,
    trace_redact: bool,
    checkpoint_out: Option<&PathBuf>,
    observed_trace: &mut Vec<TraceEvent>,
) -> Result<air_linker::SystemRunResult> {
    if observe {
        let result = air_linker::run_run_plan_parallel_with_observer_and_checkpoint(
            plan,
            store,
            base_dir,
            inputs,
            || tools.clone(),
            || models.clone(),
            |event| {
                observe_event_with_trace_file(event, log, observed_trace, trace_out, trace_redact)
            },
            |checkpoint| write_checkpoint_state(checkpoint_out, checkpoint),
        );
        if result.is_err() {
            write_partial_trace(trace_out, observed_trace, trace_redact)?;
        }
        Ok(result?)
    } else {
        Ok(
            air_linker::run_run_plan_parallel_with_observer_and_checkpoint(
                plan,
                store,
                base_dir,
                inputs,
                || tools.clone(),
                || models.clone(),
                |_| {},
                |checkpoint| write_checkpoint_state(checkpoint_out, checkpoint),
            )?,
        )
    }
}

pub(crate) fn resume_plan(options: ResumePlanOptions) -> Result<()> {
    let ResumePlanOptions {
        plan,
        profile,
        store,
        input,
        state,
        overrides,
        model_config,
        trace_out,
        trace_redact,
        trace_raw,
        state_out,
        checkpoint_out,
        log,
        example_tools,
        tool_config,
    } = options;

    let profile = match profile {
        Some(path) => Some((path.clone(), read_run_plan_profile(&path)?)),
        None => None,
    };

    let plan = plan
        .or_else(|| {
            profile
                .as_ref()
                .map(|(path, profile)| resolve_profile_path(path, &profile.plan))
        })
        .ok_or_else(|| anyhow::anyhow!("resume-plan requires a plan path or --profile"))?;
    let store = store
        .or_else(|| {
            profile
                .as_ref()
                .map(|(path, profile)| resolve_profile_path(path, &profile.store))
        })
        .ok_or_else(|| anyhow::anyhow!("resume-plan requires --store or --profile"))?;
    let input = input.or_else(|| {
        profile.as_ref().and_then(|(path, profile)| {
            profile
                .input_file
                .as_ref()
                .map(|value| resolve_profile_path(path, value))
        })
    });
    let model_config = model_config.or_else(|| {
        profile.as_ref().and_then(|(path, profile)| {
            profile
                .model_config
                .as_ref()
                .map(|value| resolve_profile_path(path, value))
        })
    });
    let tool_config = tool_config.or_else(|| {
        profile.as_ref().and_then(|(path, profile)| {
            profile
                .tool_config
                .as_ref()
                .map(|value| resolve_profile_path(path, value))
        })
    });
    let trace_out = trace_out.or_else(|| {
        profile.as_ref().and_then(|(path, profile)| {
            profile
                .trace_out
                .as_ref()
                .map(|value| resolve_profile_path(path, value))
        })
    });
    let state_out = state_out.or_else(|| {
        profile.as_ref().and_then(|(path, profile)| {
            profile
                .state_out
                .as_ref()
                .map(|value| resolve_profile_path(path, value))
        })
    });
    let checkpoint_out = checkpoint_out.or_else(|| {
        profile.as_ref().and_then(|(path, profile)| {
            profile
                .checkpoint_out
                .as_ref()
                .map(|value| resolve_profile_path(path, value))
        })
    });
    let log = log
        || profile
            .as_ref()
            .and_then(|(_, profile)| profile.log)
            .unwrap_or(false);
    let trace_redact = effective_trace_redact(trace_redact, trace_raw, profile.as_ref());
    let example_tools = example_tools
        || profile
            .as_ref()
            .and_then(|(_, profile)| profile.example_tools)
            .unwrap_or(false);

    let plan = air_linker::parse_run_plan_file(plan)?;
    let store_path = store;
    let store = air_linker::parse_module_store_file(&store_path)?;
    let base_dir = module_base_dir_for_store_path(&store, &store_path);
    let report = air_linker::validate_run_plan(&plan, &store, &base_dir);
    if !report.is_success() {
        for diagnostic in &report.diagnostics {
            eprintln!("error[{}]: {}", diagnostic.code, diagnostic.message);
        }
        std::process::exit(1);
    }

    let inputs = if let Some(input) = input {
        read_json_object(input, "--input")?
    } else if let Some((_, profile)) = &profile {
        if let Some(inputs) = &profile.inputs {
            serde_json::Map::from_iter(inputs.clone())
        } else {
            anyhow::bail!("resume-plan requires --input, profile.input_file, or profile.inputs");
        }
    } else {
        anyhow::bail!("resume-plan requires --input or --profile");
    };
    let state_file: PlanStateFile = serde_json::from_str(&fs::read_to_string(state)?)?;
    let mut resume = air_linker::ResumeState {
        module_outputs: state_file
            .module_outputs
            .into_iter()
            .map(|(module, value)| {
                let Value::Object(output) = value else {
                    anyhow::bail!("state module output {module} must be a JSON object");
                };
                Ok((module, output))
            })
            .collect::<Result<_>>()?,
        output_overrides: BTreeMap::new(),
    };
    for override_spec in overrides {
        let (endpoint, value) = parse_output_override(&override_spec)?;
        resume.output_overrides.insert(endpoint, value);
    }

    let observe = log || trace_out.is_some();
    let mut observed_trace = Vec::new();
    let tools = ToolProviderChoice::from_config(tool_config, example_tools)?;
    let result = if let Some(model_config) = model_config {
        let models = ModelProviderChoice::from_config_file(model_config)?;
        if observe {
            let result = air_linker::resume_run_plan_with_observer_and_checkpoint(
                &plan,
                &store,
                base_dir,
                inputs,
                resume,
                tools,
                models,
                |event| {
                    observe_event_with_trace_file(
                        event,
                        log,
                        &mut observed_trace,
                        trace_out.as_ref(),
                        trace_redact,
                    )
                },
                |checkpoint| write_checkpoint_state(checkpoint_out.as_ref(), checkpoint),
            );
            if result.is_err() {
                write_partial_trace(trace_out.as_ref(), &observed_trace, trace_redact)?;
            }
            result?
        } else {
            air_linker::resume_run_plan_with_observer_and_checkpoint(
                &plan,
                &store,
                base_dir,
                inputs,
                resume,
                tools,
                models,
                |_| {},
                |checkpoint| write_checkpoint_state(checkpoint_out.as_ref(), checkpoint),
            )?
        }
    } else if observe {
        let result = air_linker::resume_run_plan_with_observer_and_checkpoint(
            &plan,
            &store,
            base_dir,
            inputs,
            resume,
            tools,
            ModelProviderChoice::echo(),
            |event| {
                observe_event_with_trace_file(
                    event,
                    log,
                    &mut observed_trace,
                    trace_out.as_ref(),
                    trace_redact,
                )
            },
            |checkpoint| write_checkpoint_state(checkpoint_out.as_ref(), checkpoint),
        );
        if result.is_err() {
            write_partial_trace(trace_out.as_ref(), &observed_trace, trace_redact)?;
        }
        result?
    } else {
        air_linker::resume_run_plan_with_observer_and_checkpoint(
            &plan,
            &store,
            base_dir,
            inputs,
            resume,
            tools,
            ModelProviderChoice::echo(),
            |_| {},
            |checkpoint| write_checkpoint_state(checkpoint_out.as_ref(), checkpoint),
        )?
    };

    if let Some(trace_out) = trace_out {
        let mut trace = result.trace.clone();
        trace.push(system_return_event(result.outputs.clone()));
        write_trace(trace_out, &trace, trace_redact)?;
    }
    if let Some(state_out) = state_out {
        write_plan_state(state_out, &result)?;
    }
    println!("{}", serde_json::to_string_pretty(&result.outputs)?);

    Ok(())
}

fn parse_output_override(spec: &str) -> Result<(String, Value)> {
    let Some((endpoint, raw_value)) = spec.split_once('=') else {
        anyhow::bail!("override must use endpoint=json syntax");
    };
    if endpoint.trim().is_empty() {
        anyhow::bail!("override endpoint must not be empty");
    }
    let value =
        serde_json::from_str(raw_value).unwrap_or_else(|_| Value::String(raw_value.to_string()));
    Ok((endpoint.to_string(), value))
}

fn write_plan_state(path: PathBuf, result: &air_linker::SystemRunResult) -> Result<()> {
    let state = PlanStateFile {
        status: result.status.clone(),
        outputs: Value::Object(result.outputs.clone()),
        module_outputs: result
            .module_outputs
            .iter()
            .map(|(module, outputs)| (module.clone(), Value::Object(outputs.clone())))
            .collect(),
    };
    fs::write(path, serde_json::to_string_pretty(&state)?)?;
    Ok(())
}

pub(crate) fn write_checkpoint_state(
    path: Option<&PathBuf>,
    checkpoint: &air_linker::SystemCheckpoint,
) -> Result<(), String> {
    let Some(path) = path else {
        return Ok(());
    };

    let state = PlanStateFile {
        status: air_linker::RunStatus::InProgress {
            after: checkpoint.completed_module.clone(),
        },
        outputs: Value::Object(Default::default()),
        module_outputs: checkpoint
            .module_outputs
            .iter()
            .map(|(module, outputs)| (module.clone(), Value::Object(outputs.clone())))
            .collect(),
    };
    let content = serde_json::to_string_pretty(&state).map_err(|error| error.to_string())?;
    fs::write(path, content).map_err(|error| error.to_string())?;
    Ok(())
}

pub(crate) fn observe_event(event: &TraceEvent, log: bool, trace: &mut Vec<TraceEvent>) {
    if log {
        log_event(event);
    }
    trace.push(event.clone());
}

pub(crate) fn observe_event_with_trace_file(
    event: &TraceEvent,
    log: bool,
    trace: &mut Vec<TraceEvent>,
    trace_out: Option<&PathBuf>,
    trace_redact: bool,
) {
    observe_event(event, log, trace);
    if let Err(error) = write_partial_trace(trace_out, trace, trace_redact) {
        eprintln!(
            "[air] failed to write incremental trace after {}:{}: {error}",
            event.rule, event.action
        );
    }
}

pub(crate) fn write_partial_trace(
    trace_out: Option<&PathBuf>,
    trace: &[TraceEvent],
    trace_redact: bool,
) -> Result<()> {
    if let Some(path) = trace_out {
        write_trace(path, trace, trace_redact)?;
    }
    Ok(())
}

pub(crate) fn write_trace(
    path: impl AsRef<std::path::Path>,
    trace: &[TraceEvent],
    redact: bool,
) -> Result<()> {
    if redact {
        write_trace_jsonl_with_options(path, trace, &TraceWriteOptions::redacted())?;
    } else {
        write_trace_jsonl_with_options(path, trace, &TraceWriteOptions::raw())?;
    }
    Ok(())
}

fn log_event(event: &TraceEvent) {
    let status = match event.status {
        TraceStatus::Ok => "ok",
        TraceStatus::Error => "error",
    };
    let input = event
        .input
        .as_ref()
        .map(value_shape)
        .map(|shape| format!(" input={shape}"))
        .unwrap_or_default();
    let output = event
        .output
        .as_ref()
        .map(value_shape)
        .map(|shape| format!(" output={shape}"))
        .unwrap_or_default();
    let meta = event
        .meta
        .as_ref()
        .map(value_shape)
        .map(|shape| format!(" meta={shape}"))
        .unwrap_or_default();
    let error = event
        .error
        .as_ref()
        .map(|error| format!(" error={error}"))
        .unwrap_or_default();

    eprintln!(
        "[air] {} step={} rule={} action={} status={}{}{}{}{}",
        event.agent, event.step, event.rule, event.action, status, meta, input, output, error
    );
}

fn value_shape(value: &Value) -> String {
    match value {
        Value::Object(object) => {
            let mut entries = object
                .iter()
                .take(8)
                .map(|(key, value)| format!("{key}:{}", compact_value(value)))
                .collect::<Vec<_>>();
            if object.len() > entries.len() {
                entries.push("...".to_string());
            }
            format!("{{{}}}", entries.join(","))
        }
        value => compact_value(value),
    }
}

fn compact_value(value: &Value) -> String {
    match value {
        Value::Object(object) => format!("object({})", object.len()),
        Value::Array(values) => format!("[{}]", values.len()),
        Value::String(value) => {
            let max = 80;
            let mut preview = value.chars().take(max).collect::<String>();
            if value.chars().count() > max {
                preview.push_str("...");
            }
            format!("{preview:?}")
        }
        Value::Number(value) => value.to_string(),
        Value::Bool(value) => value.to_string(),
        Value::Null => "null".to_string(),
    }
}

pub(crate) fn replay(options: ReplayOptions) -> Result<()> {
    let ReplayOptions {
        trace,
        specialize_run_plan,
        store,
        output,
        identity_out,
        stats,
    } = options;
    let events = read_trace_jsonl(trace)?;
    if specialize_run_plan {
        let store = store
            .ok_or_else(|| anyhow::anyhow!("replay --specialize-run-plan requires --store"))?;
        let store_path = store;
        let store = air_linker::parse_module_store_file(&store_path)?;
        let base_dir = module_base_dir_for_store_path(&store, &store_path);
        let specialization = air_linker::specialize_run_plan_trace(&events, &store, &base_dir)?;
        let plan_yaml = serde_yaml::to_string(&specialization.plan)?;
        if let Some(output) = output {
            fs::write(output, plan_yaml)?;
        } else {
            print!("{plan_yaml}");
        }
        if let Some(identity_out) = identity_out {
            fs::write(
                identity_out,
                serde_json::to_string_pretty(&specialization.cache_identity)?,
            )?;
        }
    } else if stats {
        println!("{}", serde_json::to_string_pretty(&replay_stats(&events)?)?);
    } else {
        let output = replay_outputs(&events)?;
        println!("{}", serde_json::to_string_pretty(&output)?);
    }
    Ok(())
}

fn replay_stats(events: &[TraceEvent]) -> Result<Value> {
    let final_output = replay_outputs(events)?;
    Ok(json!({
        "event_count": events.len(),
        "error_count": events.iter().filter(|event| event.status == TraceStatus::Error).count(),
        "model_call_count": events.iter().filter(|event| event.action == "model_call").count(),
        "tool_call_count": events.iter().filter(|event| is_tool_call_event(event)).count(),
        "final_output": final_output,
    }))
}

fn is_tool_call_event(event: &TraceEvent) -> bool {
    matches!(
        event.action.as_str(),
        "tool_call" | "tool_dispatch" | "tool_batch_dispatch_item"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trace_event(action: &str, status: TraceStatus) -> TraceEvent {
        let error = (status == TraceStatus::Error).then(|| "failed".to_string());
        TraceEvent {
            agent: "agent".to_string(),
            step: 0,
            rule: "rule".to_string(),
            action: action.to_string(),
            input: None,
            output: None,
            meta: None,
            status,
            error,
        }
    }

    #[test]
    fn replay_stats_counts_trace_events() {
        let mut output = serde_json::Map::new();
        output.insert("ok".to_string(), json!(true));
        let events = vec![
            trace_event("model_call", TraceStatus::Ok),
            trace_event("tool_dispatch", TraceStatus::Error),
            trace_event("tool_batch_dispatch_item", TraceStatus::Ok),
            system_return_event(output),
        ];

        let stats = replay_stats(&events).unwrap();

        assert_eq!(stats["event_count"], json!(4));
        assert_eq!(stats["error_count"], json!(1));
        assert_eq!(stats["model_call_count"], json!(1));
        assert_eq!(stats["tool_call_count"], json!(2));
        assert_eq!(stats["final_output"], json!({"ok": true}));
    }

    #[test]
    fn observer_writes_incremental_trace_file() {
        let path = std::env::temp_dir().join(format!(
            "air-incremental-trace-{}-{}.jsonl",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let event = TraceEvent {
            agent: "agent".to_string(),
            step: 0,
            rule: "choose".to_string(),
            action: "model_call_start".to_string(),
            input: Some(json!({"secret": "sk-test-123", "visible": "ok"})),
            output: None,
            meta: Some(json!({"model": "code_edit_decider"})),
            status: TraceStatus::Ok,
            error: None,
        };
        let mut trace = Vec::new();

        observe_event_with_trace_file(&event, false, &mut trace, Some(&path), true);

        let content = fs::read_to_string(&path).unwrap();
        let events = read_trace_jsonl(&path).unwrap();
        let _ = fs::remove_file(&path);

        assert_eq!(trace.len(), 1);
        assert_eq!(events.len(), 1);
        assert!(content.contains("model_call_start"));
        assert!(content.contains("[AIR_REDACTED]"));
        assert!(!content.contains("sk-test-123"));
    }
}
