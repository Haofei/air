use crate::{AirSystem, LinkerError};
use air_core::AirModule;
use air_runtime::{ModelProvider, RunResult, State, ToolProvider, Vm};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExecutionBatch {
    pub(crate) group_id: Option<String>,
    pub(crate) modules: Vec<String>,
    pub(crate) max_parallel: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ModuleJob {
    pub(crate) module_id: String,
    pub(crate) module: AirModule,
    pub(crate) inputs: State,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ModuleJobResult {
    pub(crate) module_id: String,
    pub(crate) result: RunResult,
}

pub(crate) fn run_module_jobs_sequential<T, M, TF, MF>(
    jobs: Vec<ModuleJob>,
    tools_factory: &TF,
    models_factory: &MF,
) -> Result<Vec<ModuleJobResult>, LinkerError>
where
    T: ToolProvider,
    M: ModelProvider,
    TF: Fn() -> T,
    MF: Fn() -> M,
{
    let mut results = Vec::new();
    for job in jobs {
        let mut vm = Vm {
            tools: tools_factory(),
            models: models_factory(),
        };
        let result = vm
            .run(&job.module, job.inputs)
            .map_err(|source| LinkerError::Runtime {
                module: job.module_id.clone(),
                source,
            })?;
        results.push(ModuleJobResult {
            module_id: job.module_id,
            result,
        });
    }
    Ok(results)
}

pub(crate) fn run_module_jobs_parallel<T, M, TF, MF>(
    jobs: Vec<ModuleJob>,
    tools_factory: &TF,
    models_factory: &MF,
) -> Result<Vec<ModuleJobResult>, LinkerError>
where
    T: ToolProvider + Send,
    M: ModelProvider + Send,
    TF: Fn() -> T + Sync,
    MF: Fn() -> M + Sync,
{
    std::thread::scope(|scope| {
        let handles = jobs
            .into_iter()
            .map(|job| {
                scope.spawn(move || {
                    let mut vm = Vm {
                        tools: tools_factory(),
                        models: models_factory(),
                    };
                    let result =
                        vm.run(&job.module, job.inputs)
                            .map_err(|source| LinkerError::Runtime {
                                module: job.module_id.clone(),
                                source,
                            })?;
                    Ok(ModuleJobResult {
                        module_id: job.module_id,
                        result,
                    })
                })
            })
            .collect::<Vec<_>>();

        handles
            .into_iter()
            .map(|handle| {
                handle
                    .join()
                    .map_err(|_| LinkerError::ParallelWorkerPanic)?
            })
            .collect()
    })
}

pub(crate) fn execution_batches(system: &AirSystem) -> Result<Vec<ExecutionBatch>, LinkerError> {
    let Some(schedule) = &system.schedule else {
        return topological_order(system).map(|order| {
            order
                .into_iter()
                .map(|module| ExecutionBatch {
                    group_id: None,
                    modules: vec![module],
                    max_parallel: 1,
                })
                .collect()
        });
    };

    let mut indegree: BTreeMap<String, usize> = system
        .modules
        .keys()
        .map(|module| (module.clone(), 0))
        .collect();
    let mut outgoing: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for edge in &system.edges {
        if !system.modules.contains_key(&edge.from) {
            return Err(LinkerError::UnknownModule(edge.from.clone()));
        }
        if !system.modules.contains_key(&edge.to) {
            return Err(LinkerError::UnknownModule(edge.to.clone()));
        }

        *indegree.entry(edge.to.clone()).or_default() += 1;
        outgoing
            .entry(edge.from.clone())
            .or_default()
            .push(edge.to.clone());
    }

    let mut remaining: BTreeSet<String> = system.modules.keys().cloned().collect();
    let mut completed = Vec::new();
    let mut batches = Vec::new();
    let default_max_parallel = schedule.max_parallel.unwrap_or(usize::MAX).max(1);

    while !remaining.is_empty() {
        let ready = remaining
            .iter()
            .filter(|module| indegree.get(*module).copied().unwrap_or_default() == 0)
            .cloned()
            .collect::<BTreeSet<_>>();
        if ready.is_empty() {
            return Err(LinkerError::Cycle);
        }

        let scheduled_group = schedule.groups.iter().find(|group| {
            let group_nodes = group.nodes.iter().cloned().collect::<BTreeSet<_>>();
            !group_nodes.is_empty()
                && group_nodes.iter().any(|node| remaining.contains(node))
                && group_nodes
                    .iter()
                    .filter(|node| remaining.contains(*node))
                    .all(|node| ready.contains(node))
        });

        let (group_id, max_parallel, selected) = if let Some(group) = scheduled_group {
            let group_max_parallel = group.max_parallel.unwrap_or(default_max_parallel).max(1);
            let limit = group_max_parallel.min(default_max_parallel);
            (
                Some(group.id.clone()),
                limit,
                group
                    .nodes
                    .iter()
                    .filter(|node| remaining.contains(*node))
                    .take(limit)
                    .cloned()
                    .collect::<Vec<_>>(),
            )
        } else {
            (None, 1, vec![ready.iter().next().expect("ready").clone()])
        };

        for module in &selected {
            remaining.remove(module);
            completed.push(module.clone());
            for next in outgoing.get(module).into_iter().flatten() {
                let degree = indegree.get_mut(next).expect("known module");
                *degree -= 1;
            }
        }

        batches.push(ExecutionBatch {
            group_id,
            modules: selected,
            max_parallel,
        });
    }

    if completed.len() != system.modules.len() {
        return Err(LinkerError::Cycle);
    }

    Ok(batches)
}

pub(crate) fn direct_predecessors(system: &AirSystem) -> BTreeMap<String, BTreeSet<String>> {
    let mut predecessors: BTreeMap<String, BTreeSet<String>> = system
        .modules
        .keys()
        .map(|module| (module.clone(), BTreeSet::new()))
        .collect();

    for edge in &system.edges {
        predecessors
            .entry(edge.to.clone())
            .or_default()
            .insert(edge.from.clone());
    }

    predecessors
}

pub(crate) fn topological_order(system: &AirSystem) -> Result<Vec<String>, LinkerError> {
    let mut indegree: BTreeMap<String, usize> = system
        .modules
        .keys()
        .map(|module| (module.clone(), 0))
        .collect();
    let mut outgoing: BTreeMap<String, Vec<String>> = BTreeMap::new();

    for edge in &system.edges {
        if !system.modules.contains_key(&edge.from) {
            return Err(LinkerError::UnknownModule(edge.from.clone()));
        }
        if !system.modules.contains_key(&edge.to) {
            return Err(LinkerError::UnknownModule(edge.to.clone()));
        }

        *indegree.entry(edge.to.clone()).or_default() += 1;
        outgoing
            .entry(edge.from.clone())
            .or_default()
            .push(edge.to.clone());
    }

    let mut ready: Vec<_> = indegree
        .iter()
        .filter_map(|(module, degree)| (*degree == 0).then_some(module.clone()))
        .collect();
    let mut order = Vec::new();

    while let Some(module) = ready.pop() {
        order.push(module.clone());
        for next in outgoing.get(&module).into_iter().flatten() {
            let degree = indegree.get_mut(next).expect("known module");
            *degree -= 1;
            if *degree == 0 {
                ready.push(next.clone());
            }
        }
    }

    if order.len() != system.modules.len() {
        return Err(LinkerError::Cycle);
    }

    Ok(order)
}
