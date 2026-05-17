use crate::{AirSystem, LinkerError, ModuleStore, RunPlan};
use serde::de::DeserializeOwned;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

pub fn parse_system_file(path: impl AsRef<Path>) -> Result<AirSystem, LinkerError> {
    parse_yaml_file(path)
}

pub fn parse_module_store_file(path: impl AsRef<Path>) -> Result<ModuleStore, LinkerError> {
    let path = path.as_ref();
    let mut visited = BTreeSet::new();
    parse_module_store_file_inner(path, true, &mut visited)
}

fn parse_module_store_file_inner(
    path: &Path,
    preserve_local_paths: bool,
    visited: &mut BTreeSet<PathBuf>,
) -> Result<ModuleStore, LinkerError> {
    let canonical = path.canonicalize().map_err(|source| LinkerError::Read {
        path: path.display().to_string(),
        source,
    })?;
    if !visited.insert(canonical.clone()) {
        return Err(LinkerError::StoreImportCycle(
            canonical.display().to_string(),
        ));
    }

    let mut store: ModuleStore = parse_yaml_file(path)?;
    let store_dir = path.parent().unwrap_or_else(|| Path::new("."));
    let imports = store.imports.clone();

    let mut merged = ModuleStore {
        store: store.store.clone(),
        imports: Vec::new(),
        modules: Default::default(),
        recipes: Vec::new(),
    };

    for import in imports {
        let import_path = if import.is_absolute() {
            import
        } else {
            store_dir.join(import)
        };
        let imported = parse_module_store_file_inner(&import_path, false, visited)?;
        merge_store_import(&mut merged, imported, &import_path)?;
    }

    if !preserve_local_paths {
        absolutize_module_paths(&mut store, store_dir)?;
    }

    merge_store_import(&mut merged, store, path)?;
    visited.remove(&canonical);
    Ok(merged)
}

fn absolutize_module_paths(store: &mut ModuleStore, store_dir: &Path) -> Result<(), LinkerError> {
    for module_ref in store.modules.values_mut() {
        if module_ref.path.is_absolute() {
            continue;
        }
        let candidate = store_dir.join(&module_ref.path);
        module_ref.path = candidate
            .canonicalize()
            .map_err(|source| LinkerError::Read {
                path: candidate.display().to_string(),
                source,
            })?;
    }
    Ok(())
}

fn merge_store_import(
    target: &mut ModuleStore,
    imported: ModuleStore,
    import_path: &Path,
) -> Result<(), LinkerError> {
    let import = import_path.display().to_string();
    for (module, module_ref) in imported.modules {
        if target.modules.contains_key(&module) {
            return Err(LinkerError::DuplicateStoreModule {
                module,
                import: import.clone(),
            });
        }
        target.modules.insert(module, module_ref);
    }
    let mut recipe_ids = target
        .recipes
        .iter()
        .map(|recipe| recipe.id.clone())
        .collect::<BTreeSet<_>>();
    for recipe in imported.recipes {
        if !recipe_ids.insert(recipe.id.clone()) {
            return Err(LinkerError::DuplicateStoreRecipe {
                recipe: recipe.id,
                import: import.clone(),
            });
        }
        target.recipes.push(recipe);
    }
    Ok(())
}

pub fn parse_run_plan_file(path: impl AsRef<Path>) -> Result<RunPlan, LinkerError> {
    parse_yaml_file(path)
}

fn parse_yaml_file<T>(path: impl AsRef<Path>) -> Result<T, LinkerError>
where
    T: DeserializeOwned,
{
    let path = path.as_ref();
    let source = fs::read_to_string(path).map_err(|source| LinkerError::Read {
        path: path.display().to_string(),
        source,
    })?;
    serde_yaml::from_str(&source).map_err(|error| air_parser::ParseError::Yaml(error).into())
}

pub fn resolve_module_path(
    base_dir: impl AsRef<Path>,
    module_id: &str,
    module_path: impl AsRef<Path>,
) -> Result<PathBuf, LinkerError> {
    let base_dir = base_dir.as_ref();
    let module_path = module_path.as_ref();
    let base = base_dir
        .canonicalize()
        .map_err(|source| LinkerError::Read {
            path: base_dir.display().to_string(),
            source,
        })?;
    let candidate = if module_path.is_absolute() {
        module_path.to_path_buf()
    } else {
        base_dir.join(module_path)
    };
    let canonical = candidate
        .canonicalize()
        .map_err(|source| LinkerError::Read {
            path: candidate.display().to_string(),
            source,
        })?;
    if !canonical.starts_with(&base) {
        return Err(LinkerError::ModulePathOutsideBase {
            module: module_id.to_string(),
            path: canonical.display().to_string(),
            base_dir: base.display().to_string(),
        });
    }
    Ok(canonical)
}
