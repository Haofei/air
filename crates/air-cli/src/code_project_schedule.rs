use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug)]
pub(crate) struct CodeProjectScheduledTask {
    pub(crate) id: String,
    pub(crate) depends_on: Vec<String>,
    pub(crate) definition: Value,
}

pub(crate) fn code_project_task_ids(tasks: &[Value]) -> Vec<Value> {
    tasks
        .iter()
        .filter_map(|task| task.get("id").and_then(Value::as_str))
        .map(|id| Value::String(id.to_string()))
        .collect()
}

pub(crate) fn code_project_task_schedule(tasks: &[Value]) -> Result<Vec<CodeProjectScheduledTask>> {
    let mut parsed = Vec::new();
    let mut id_to_index = HashMap::new();
    for (index, task) in tasks.iter().enumerate() {
        let id = task
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.trim().is_empty())
            .with_context(|| format!("planned task at index {index} is missing non-empty id"))?
            .to_string();
        if id_to_index.insert(id.clone(), index).is_some() {
            bail!("planned task id {id:?} is duplicated");
        }
        let depends_on = task
            .get("depends_on")
            .and_then(Value::as_array)
            .map(|deps| {
                deps.iter()
                    .enumerate()
                    .map(|(dep_index, dep)| {
                        dep.as_str()
                            .filter(|dep| !dep.trim().is_empty())
                            .map(str::to_string)
                            .with_context(|| {
                                format!(
                                    "planned task {id:?} depends_on[{dep_index}] is not a non-empty string"
                                )
                            })
                    })
                    .collect::<Result<Vec<_>>>()
            })
            .transpose()?
            .unwrap_or_default();
        parsed.push(CodeProjectScheduledTask {
            id,
            depends_on,
            definition: task.clone(),
        });
    }

    for task in &parsed {
        for dependency in &task.depends_on {
            if !id_to_index.contains_key(dependency) {
                bail!(
                    "planned task {:?} depends on missing task {:?}",
                    task.id,
                    dependency
                );
            }
        }
    }

    let mut remaining = (0..parsed.len()).collect::<Vec<_>>();
    let mut scheduled = Vec::new();
    let mut completed_ids = HashSet::new();
    while !remaining.is_empty() {
        let Some(ready_position) = remaining.iter().position(|index| {
            parsed[*index]
                .depends_on
                .iter()
                .all(|dependency| completed_ids.contains(dependency))
        }) else {
            let blocked = remaining
                .iter()
                .map(|index| parsed[*index].id.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            bail!("planned task dependency cycle blocks: {blocked}");
        };
        let index = remaining.remove(ready_position);
        let task = parsed[index].clone();
        completed_ids.insert(task.id.clone());
        scheduled.push(task);
    }

    Ok(scheduled)
}
