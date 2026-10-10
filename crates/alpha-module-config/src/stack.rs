//! Layer merging with per-leaf provenance.

use std::collections::BTreeMap;

use serde::de::DeserializeOwned;

use crate::source::ConfigSource;

/// Errors from configuration resolution.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// The merged table does not match the requested type.
    #[error("failed to decode configuration: {0}")]
    Decode(#[from] toml::de::Error),
}

/// Ordered configuration layers.
#[derive(Clone, Debug, Default)]
pub struct ConfigStack {
    layers: Vec<(ConfigSource, toml::Table)>,
}

impl ConfigStack {
    /// Create an empty stack.
    pub fn new() -> Self {
        Self::default()
    }

    /// Push one layer. Layers with the same source keep insertion order.
    pub fn push(&mut self, source: ConfigSource, table: toml::Table) {
        self.layers.push((source, table));
    }

    /// Merge all layers into one resolved configuration.
    pub fn resolve(&self) -> ResolvedConfig {
        let mut layers = self.layers.clone();
        layers.sort_by_key(|(source, _)| *source);

        let mut table = toml::Table::new();
        let mut provenance = BTreeMap::new();
        for (source, layer) in layers {
            merge_table(&mut table, &layer, "", source, &mut provenance);
        }
        ResolvedConfig { table, provenance }
    }
}

/// A merged configuration with provenance for every leaf.
#[derive(Clone, Debug)]
pub struct ResolvedConfig {
    table: toml::Table,
    provenance: BTreeMap<String, ConfigSource>,
}

impl ResolvedConfig {
    /// The merged table.
    pub fn as_table(&self) -> &toml::Table {
        &self.table
    }

    /// Look up a dotted path.
    pub fn get(&self, path: &str) -> Option<&toml::Value> {
        let mut current = self.table.get(path.split('.').next()?)?;
        for segment in path.split('.').skip(1) {
            current = current.as_table()?.get(segment)?;
        }
        Some(current)
    }

    /// Which layer last set the value at a dotted path.
    pub fn source_of(&self, path: &str) -> Option<ConfigSource> {
        self.provenance.get(path).copied()
    }

    /// Decode the merged table into a typed configuration.
    pub fn decode<T: DeserializeOwned>(&self) -> Result<T, ConfigError> {
        toml::Value::Table(self.table.clone())
            .try_into()
            .map_err(ConfigError::from)
    }
}

fn merge_table(
    target: &mut toml::Table,
    source: &toml::Table,
    prefix: &str,
    origin: ConfigSource,
    provenance: &mut BTreeMap<String, ConfigSource>,
) {
    for (key, value) in source {
        let path = join_path(prefix, key);
        match value {
            toml::Value::Table(source_table) => {
                let entry = target
                    .entry(key.clone())
                    .or_insert_with(|| toml::Value::Table(toml::Table::new()));
                if let toml::Value::Table(target_table) = entry {
                    merge_table(target_table, source_table, &path, origin, provenance);
                } else {
                    clear_prefix(&path, provenance);
                    let mut replacement = toml::Table::new();
                    merge_table(&mut replacement, source_table, &path, origin, provenance);
                    *entry = toml::Value::Table(replacement);
                }
            }
            other => {
                clear_prefix(&path, provenance);
                target.insert(key.clone(), other.clone());
                provenance.insert(path, origin);
            }
        }
    }
}

fn join_path(prefix: &str, key: &str) -> String {
    if prefix.is_empty() {
        key.to_owned()
    } else {
        format!("{prefix}.{key}")
    }
}

fn clear_prefix(prefix: &str, provenance: &mut BTreeMap<String, ConfigSource>) {
    let nested = format!("{prefix}.");
    provenance.retain(|key, _| key != prefix && !key.starts_with(&nested));
}
