use crate::{AirSystem, LinkExpr, LinkerError};
use air_core::TypeSpec;
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Endpoint {
    pub(crate) module: String,
    pub(crate) field: String,
    pub(crate) path: Vec<String>,
    pub(crate) append: bool,
}

impl Endpoint {
    pub(crate) fn display_field(&self) -> String {
        if self.path.is_empty() {
            return self.field.clone();
        }
        format!("{}.{}", self.field, self.path.join("."))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedConnection {
    pub(crate) source: LinkExpr,
    pub(crate) to_field: String,
    pub(crate) mode: ConnectionMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConnectionMode {
    Direct,
    Append,
}

pub(crate) fn parse_endpoint(endpoint: &str) -> Result<Endpoint, LinkerError> {
    let Some((module, rest)) = endpoint.split_once('.') else {
        return Err(LinkerError::InvalidEndpoint(endpoint.to_string()));
    };
    let append = rest.ends_with("[]");
    let rest = rest.strip_suffix("[]").unwrap_or(rest);
    let normalized = normalize_endpoint_path(rest)?;
    let mut segments = normalized.split('.');
    let field = segments.next().unwrap_or_default();
    let path = segments.map(str::to_string).collect::<Vec<_>>();
    if module.is_empty() || field.is_empty() || path.iter().any(|segment| segment.is_empty()) {
        return Err(LinkerError::InvalidEndpoint(endpoint.to_string()));
    }
    Ok(Endpoint {
        module: module.to_string(),
        field: field.to_string(),
        path,
        append,
    })
}

pub(crate) fn normalize_endpoint_path(path: &str) -> Result<String, LinkerError> {
    let mut normalized = String::new();
    let mut rest = path;
    while let Some(start) = rest.find('[') {
        let (before, after_start) = rest.split_at(start);
        normalized.push_str(before);
        let after_start = &after_start[1..];
        let Some(end) = after_start.find(']') else {
            return Err(LinkerError::InvalidEndpoint(path.to_string()));
        };
        let (index, after_end) = after_start.split_at(end);
        if index.is_empty() || !index.chars().all(|ch| ch.is_ascii_digit()) {
            return Err(LinkerError::InvalidEndpoint(path.to_string()));
        }
        normalized.push('.');
        normalized.push_str(index);
        rest = &after_end[1..];
    }
    if rest.contains(']') {
        return Err(LinkerError::InvalidEndpoint(path.to_string()));
    }
    normalized.push_str(rest);
    Ok(normalized)
}

pub(crate) fn types_compatible(from: &TypeSpec, to: &TypeSpec) -> bool {
    from.kind_name() == to.kind_name()
}

pub(crate) fn nested_type<'a>(spec: &'a TypeSpec, path: &[String]) -> Option<&'a TypeSpec> {
    let mut current = spec;
    for segment in path {
        let TypeSpec::Detailed(detailed) = current else {
            return None;
        };
        if detailed.kind.kind_name() == "array" {
            segment.parse::<usize>().ok()?;
            current = detailed.items.as_deref()?;
        } else {
            current = detailed.properties.get(segment)?;
        }
    }
    Some(current)
}

pub(crate) fn read_json_path<'a>(
    value: &'a serde_json::Value,
    path: &[String],
) -> Option<&'a serde_json::Value> {
    let mut current = value;
    for segment in path {
        if let Some(object) = current.as_object() {
            current = object.get(segment)?;
        } else if let Some(array) = current.as_array() {
            current = array.get(segment.parse::<usize>().ok()?)?;
        } else {
            return None;
        }
    }
    Some(current)
}

pub(crate) fn set_json_path(
    value: &mut serde_json::Value,
    path: &[String],
    override_value: serde_json::Value,
) -> Option<()> {
    if path.is_empty() {
        *value = override_value;
        return Some(());
    }

    let mut current = value;
    for segment in &path[..path.len() - 1] {
        current = match current {
            serde_json::Value::Object(object) => object.get_mut(segment)?,
            serde_json::Value::Array(array) => array.get_mut(segment.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }

    let last = path.last()?;
    match current {
        serde_json::Value::Object(object) => {
            object.insert(last.clone(), override_value);
            Some(())
        }
        serde_json::Value::Array(array) => {
            let index = last.parse::<usize>().ok()?;
            let slot = array.get_mut(index)?;
            *slot = override_value;
            Some(())
        }
        _ => None,
    }
}

pub(crate) fn is_array_type(spec: &TypeSpec) -> bool {
    spec.kind_name() == "array"
}

pub(crate) fn array_item_type(spec: &TypeSpec) -> Option<&TypeSpec> {
    match spec {
        TypeSpec::Detailed(detailed) if detailed.kind.kind_name() == "array" => {
            detailed.items.as_deref()
        }
        _ => None,
    }
}

pub(crate) fn array_bounds(spec: &TypeSpec) -> (Option<usize>, Option<usize>) {
    match spec {
        TypeSpec::Detailed(detailed) if detailed.kind.kind_name() == "array" => {
            (detailed.min_items, detailed.max_items)
        }
        _ => (None, None),
    }
}

pub(crate) fn incoming_connections(
    system: &AirSystem,
) -> Result<BTreeMap<String, Vec<ParsedConnection>>, LinkerError> {
    let mut incoming: BTreeMap<String, Vec<ParsedConnection>> = BTreeMap::new();
    for connection in &system.connect {
        let to = parse_endpoint(&connection.to)?;
        let source =
            crate::connection_source_expr(connection).map_err(LinkerError::InvalidEndpoint)?;
        incoming
            .entry(to.module.clone())
            .or_default()
            .push(ParsedConnection {
                source,
                to_field: to.field,
                mode: if to.append {
                    ConnectionMode::Append
                } else {
                    ConnectionMode::Direct
                },
            });
    }
    Ok(incoming)
}
