use air_runtime::{
    read_trace_jsonl, replay_outputs, write_trace_jsonl_with_options, TraceEvent, TraceStatus,
    TraceWriteOptions,
};
use anyhow::Result;
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};

pub(crate) struct ReplayOptions {
    pub(crate) trace: PathBuf,
    pub(crate) stats: bool,
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
    path: impl AsRef<Path>,
    trace: &[TraceEvent],
    redact: bool,
) -> Result<()> {
    let path = path.as_ref();
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    if redact {
        write_trace_jsonl_with_options(path, trace, &TraceWriteOptions::redacted())?;
    } else {
        write_trace_jsonl_with_options(path, trace, &TraceWriteOptions::raw())?;
    }
    Ok(())
}

pub(crate) fn replay(options: ReplayOptions) -> Result<()> {
    let events = read_trace_jsonl(options.trace)?;
    if options.stats {
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
        "tool_call" | "tool_batch_dispatch_item"
    )
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
        "[{status}] {} step={} rule={} action={}{}{}{}{}",
        event.agent, event.step, event.rule, event.action, input, output, meta, error
    );
}

fn value_shape(value: &Value) -> String {
    match value {
        Value::Object(object) => {
            let keys = object.keys().take(5).cloned().collect::<Vec<_>>();
            format!("object(keys={keys:?})")
        }
        Value::Array(array) => format!("array(len={})", array.len()),
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

#[cfg(test)]
mod tests {
    use super::*;
    use air_runtime::{read_trace_jsonl, TraceStatus};

    #[test]
    fn write_trace_creates_parent_dirs() {
        let path = std::env::temp_dir()
            .join(format!("air-trace-{}", std::process::id()))
            .join("trace.jsonl");
        let event = TraceEvent {
            agent: "$system".to_string(),
            step: 0,
            rule: "test".to_string(),
            action: "return".to_string(),
            input: None,
            output: Some(json!({"ok": true})),
            meta: None,
            status: TraceStatus::Ok,
            error: None,
        };

        write_trace(&path, &[event], true).unwrap();
        let events = read_trace_jsonl(&path).unwrap();
        assert_eq!(events.len(), 1);

        let _ = fs::remove_dir_all(path.parent().unwrap());
    }
}
