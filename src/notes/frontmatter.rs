use serde_yaml::Value as YamlValue;
use std::collections::BTreeSet;

pub(crate) fn extract_linear_id_from_frontmatter(
    frontmatter: &serde_yaml::Mapping,
) -> Option<String> {
    let value = frontmatter.get(YamlValue::String("linear_id".to_string()))?;
    let value = yaml_string(value)?;
    let value = value.trim();

    if let Some(rest) = value.strip_prefix('[')
        && let Some((label, _)) = rest.split_once(']')
    {
        let label = label.trim();
        if !label.is_empty() {
            return Some(label.to_string());
        }
    }

    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

pub(crate) fn normalize_frontmatter_key(key: &str) -> String {
    match key.trim().to_lowercase().as_str() {
        "labels" | "label" => "tags".to_string(),
        "state" => "status".to_string(),
        other => other.to_string(),
    }
}

pub(crate) fn yaml_string(value: &YamlValue) -> Option<String> {
    match value {
        YamlValue::String(value) => Some(value.trim().to_string()),
        YamlValue::Number(value) => Some(value.to_string()),
        YamlValue::Bool(value) => Some(value.to_string()),
        _ => None,
    }
}

pub(crate) fn yaml_string_list(value: &YamlValue) -> Option<Vec<String>> {
    match value {
        YamlValue::Sequence(values) => Some(
            values
                .iter()
                .filter_map(yaml_string)
                .map(|value| value.replace(' ', "-"))
                .collect(),
        ),
        YamlValue::String(value) => Some(
            value
                .split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(|value| value.replace(' ', "-"))
                .collect(),
        ),
        _ => None,
    }
}

pub(crate) fn normalize_project_name(value: &str) -> Option<String> {
    let trimmed = value.trim();
    let trimmed = trimmed
        .strip_prefix("[[")
        .and_then(|value| value.strip_suffix("]]"))
        .unwrap_or(trimmed)
        .trim();

    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

pub(crate) fn override_frontmatter_value(content: &str, key: &str, value: YamlValue) -> String {
    let Some((frontmatter, body)) = crate::split_frontmatter(content) else {
        return content.to_string();
    };
    let Some(mut map) = parse_frontmatter_map(frontmatter) else {
        return content.to_string();
    };
    let key_value = YamlValue::String(key.to_string());
    if !map.contains_key(&key_value) {
        return content.to_string();
    }

    map.insert(key_value, value);
    let yaml = match serde_yaml::to_string(&map) {
        Ok(yaml) => yaml,
        Err(_) => return content.to_string(),
    };

    format!("---\n{}---\n{}", yaml, body)
}

pub(crate) fn collect_frontmatter_keys(
    left: &serde_yaml::Mapping,
    right: &serde_yaml::Mapping,
    ignored_properties: &[String],
) -> Vec<String> {
    let mut keys = left
        .keys()
        .chain(right.keys())
        .filter_map(|key| match key {
            YamlValue::String(key) => Some(normalize_frontmatter_key(key)),
            _ => None,
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();

    keys.retain(|key| {
        key != "ignored_properties"
            && !ignored_properties
                .iter()
                .any(|ignored| normalize_frontmatter_key(ignored) == *key)
    });

    keys
}

pub(crate) fn extract_ignored_properties(content: &str) -> Vec<String> {
    let Some((frontmatter, _)) = crate::split_frontmatter(content) else {
        return Vec::new();
    };
    let Some(map) = parse_frontmatter_map(frontmatter) else {
        return Vec::new();
    };

    let ignored = map.get(YamlValue::String("ignored_properties".to_string()));
    match ignored {
        Some(YamlValue::String(value)) => value
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string)
            .collect(),
        Some(YamlValue::Sequence(values)) => values
            .iter()
            .filter_map(|value| match value {
                YamlValue::String(value) => Some(value.trim().to_string()),
                _ => None,
            })
            .filter(|value| !value.is_empty())
            .collect(),
        _ => Vec::new(),
    }
}

pub(crate) fn parse_frontmatter_map(frontmatter: &str) -> Option<serde_yaml::Mapping> {
    let body = frontmatter.strip_prefix("---\n")?;
    let body = body
        .strip_suffix("---\n")
        .or_else(|| body.strip_suffix("\n---"))
        .or_else(|| body.strip_suffix("---"))?;
    let yaml = serde_yaml::from_str::<YamlValue>(body.trim()).ok()?;
    yaml.as_mapping().cloned()
}

pub(crate) fn render_yaml_value_diff(prefix: char, key: &str, value: &YamlValue) -> Vec<String> {
    match value {
        YamlValue::Sequence(sequence) => {
            let mut lines = vec![format!("{prefix} {key}:")];
            for item in sequence {
                lines.push(format!("{prefix}   - {}", yaml_scalar_for_diff(item)));
            }
            lines
        }
        _ => vec![format!("{prefix} {key}: {}", yaml_scalar_for_diff(value))],
    }
}

pub(crate) fn render_modified_yaml_value_diff(
    key: &str,
    old: &YamlValue,
    new: &YamlValue,
) -> Vec<String> {
    match (old, new) {
        (YamlValue::Sequence(old_seq), YamlValue::Sequence(new_seq)) => {
            let old_items = old_seq.iter().map(yaml_scalar_for_diff).collect::<Vec<_>>();
            let new_items = new_seq.iter().map(yaml_scalar_for_diff).collect::<Vec<_>>();

            let removed = old_items
                .iter()
                .filter(|item| !new_items.contains(item))
                .cloned()
                .collect::<Vec<_>>();
            let added = new_items
                .iter()
                .filter(|item| !old_items.contains(item))
                .cloned()
                .collect::<Vec<_>>();

            let mut lines = vec![format!("~ {key}:")];
            for item in removed {
                lines.push(format!("-   - {item}"));
            }
            for item in added {
                lines.push(format!("+   - {item}"));
            }
            lines
        }
        _ => vec![format!(
            "~ {key}: {} -> {}",
            yaml_scalar_for_diff(old),
            yaml_scalar_for_diff(new)
        )],
    }
}

pub(crate) fn yaml_scalar_for_diff(value: &YamlValue) -> String {
    match value {
        YamlValue::String(text) => format!("\"{}\"", text),
        _ => serde_yaml::to_string(value)
            .unwrap_or_else(|_| "<unrenderable>".to_string())
            .trim()
            .to_string(),
    }
}
