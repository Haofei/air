use std::collections::hash_map::DefaultHasher;
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::time::UNIX_EPOCH;

use air_runtime::RuntimeError;
use globset::{Glob, GlobSet, GlobSetBuilder};

pub(super) const COMMAND_SNAPSHOT_MAX_HASH_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CommandWorkspaceFile {
    pub bytes: u64,
    pub modified_ms: u128,
    pub content_hash: Option<u64>,
}

#[derive(Debug, Clone, Default)]
pub(super) struct CommandWorkspaceSnapshot {
    files: BTreeMap<String, CommandWorkspaceFile>,
}

impl CommandWorkspaceSnapshot {
    pub(super) fn capture(root: &Path, ignore: &GlobSet) -> Self {
        let mut snapshot = Self::default();
        collect_workspace_files(root, root, ignore, &mut snapshot.files);
        snapshot
    }

    pub(super) fn changed_files(&self, after: &Self) -> Vec<String> {
        self.files
            .keys()
            .chain(after.files.keys())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .filter(|path| self.files.get(*path) != after.files.get(*path))
            .cloned()
            .collect()
    }
}

pub(super) fn collect_workspace_files(
    root: &Path,
    dir: &Path,
    ignore: &GlobSet,
    files: &mut BTreeMap<String, CommandWorkspaceFile>,
) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(relative) = path.strip_prefix(root) else {
            continue;
        };
        if ignore.is_match(relative) {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if metadata.is_dir() {
            collect_workspace_files(root, &path, ignore, files);
            continue;
        }
        if !metadata.is_file() {
            continue;
        }
        let relative = relative.to_string_lossy().replace('\\', "/");
        files.insert(
            relative,
            CommandWorkspaceFile {
                bytes: metadata.len(),
                modified_ms: metadata
                    .modified()
                    .ok()
                    .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                    .map(|duration| duration.as_millis())
                    .unwrap_or_default(),
                content_hash: command_file_hash(&path, metadata.len()),
            },
        );
    }
}

pub(super) fn workspace_snapshot_ignore_set(patterns: &[String]) -> Result<GlobSet, RuntimeError> {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        builder.add(Glob::new(pattern).map_err(|error| {
            RuntimeError::Provider(format!(
                "invalid workspace_snapshot_ignore glob {pattern:?}: {error}"
            ))
        })?);
    }
    builder.build().map_err(|error| {
        RuntimeError::Provider(format!(
            "invalid workspace_snapshot_ignore glob set: {error}"
        ))
    })
}

pub(super) fn command_file_hash(path: &Path, bytes: u64) -> Option<u64> {
    if bytes > COMMAND_SNAPSHOT_MAX_HASH_BYTES {
        return None;
    }
    let content = fs::read(path).ok()?;
    let mut hasher = DefaultHasher::new();
    content.hash(&mut hasher);
    Some(hasher.finish())
}
