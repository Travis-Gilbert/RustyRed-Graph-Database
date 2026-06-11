use std::collections::BTreeMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::commands::RustyredResponse;
use crate::fulltext::{
    FullTextBackend, FullTextBackendError, FullTextDesignation, FullTextIndex,
    FULLTEXT_BACKEND_HAND_ROLLED, FULLTEXT_BACKEND_TANTIVY,
};
use crate::spatial::{
    SpatialBackend, SpatialDesignation, SpatialError, SpatialIndex, SPATIAL_BACKEND_H3,
    SPATIAL_BACKEND_S2,
};

pub type SpatialBackendFactory =
    fn(SpatialDesignation) -> Result<Box<dyn SpatialBackend>, SpatialError>;
pub type FullTextBackendFactory =
    fn(FullTextDesignation) -> Result<Box<dyn FullTextBackend>, FullTextBackendError>;
pub type PluginOperationHandler = fn(PluginOperationContext<'_>, Value) -> RustyredResponse;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum PluginCapabilityKind {
    Designation,
    Encoder,
    Index,
    Operation,
    Hook,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PluginCapability {
    pub kind: PluginCapabilityKind,
    pub name: String,
}

#[derive(Clone)]
pub struct SpatialBackendRegistration {
    pub name: &'static str,
    pub aliases: &'static [&'static str],
    pub constructor: SpatialBackendFactory,
}

impl std::fmt::Debug for SpatialBackendRegistration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SpatialBackendRegistration")
            .field("name", &self.name)
            .field("aliases", &self.aliases)
            .finish_non_exhaustive()
    }
}

#[derive(Clone)]
pub struct FullTextBackendRegistration {
    pub name: &'static str,
    pub aliases: &'static [&'static str],
    pub constructor: FullTextBackendFactory,
}

impl std::fmt::Debug for FullTextBackendRegistration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FullTextBackendRegistration")
            .field("name", &self.name)
            .field("aliases", &self.aliases)
            .finish_non_exhaustive()
    }
}

#[derive(Clone)]
pub struct PluginOperationRegistration {
    pub command: &'static str,
    pub summary: &'static str,
    pub handler: PluginOperationHandler,
}

impl std::fmt::Debug for PluginOperationRegistration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PluginOperationRegistration")
            .field("command", &self.command)
            .field("summary", &self.summary)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug)]
pub struct PluginOperationContext<'a> {
    pub command: &'a str,
    pub state_hash: String,
}

pub trait RustyRedPlugin: Send + Sync + std::fmt::Debug {
    fn name(&self) -> &'static str;

    fn capabilities(&self) -> Vec<PluginCapability> {
        Vec::new()
    }

    fn spatial_backends(&self) -> Vec<SpatialBackendRegistration> {
        Vec::new()
    }

    fn fulltext_backends(&self) -> Vec<FullTextBackendRegistration> {
        Vec::new()
    }

    fn operations(&self) -> Vec<PluginOperationRegistration> {
        Vec::new()
    }
}

#[derive(Clone, Debug, Default)]
pub struct PluginRegistry {
    plugins: Vec<Arc<dyn RustyRedPlugin>>,
    spatial_backends: BTreeMap<String, SpatialBackendRegistration>,
    fulltext_backends: BTreeMap<String, FullTextBackendRegistration>,
    operations: BTreeMap<String, PluginOperationRegistration>,
}

impl PluginRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_builtin_plugins() -> Self {
        let mut registry = Self::new();
        registry.register(CoreSpatialPlugin);
        registry.register(CoreFullTextPlugin);
        registry.register(CoreOperationsPlugin);
        #[cfg(feature = "geometry")]
        registry.register(crate::geometry::GeometryPlugin);
        registry
    }

    pub fn register(&mut self, plugin: impl RustyRedPlugin + 'static) {
        let plugin = Arc::new(plugin);
        for backend in plugin.spatial_backends() {
            self.insert_spatial_backend(backend);
        }
        for backend in plugin.fulltext_backends() {
            self.insert_fulltext_backend(backend);
        }
        for operation in plugin.operations() {
            self.operations
                .insert(normalize_command(operation.command), operation);
        }
        self.plugins.push(plugin);
    }

    pub fn plugins(&self) -> Vec<&dyn RustyRedPlugin> {
        self.plugins
            .iter()
            .map(|plugin| plugin.as_ref() as &dyn RustyRedPlugin)
            .collect()
    }

    pub fn capabilities(&self) -> Vec<PluginCapability> {
        self.plugins
            .iter()
            .flat_map(|plugin| plugin.capabilities())
            .collect()
    }

    pub fn spatial_backend(&self, raw: &str) -> Option<&SpatialBackendRegistration> {
        let key = normalize_backend_name(raw);
        self.spatial_backends.get(&key)
    }

    pub fn fulltext_backend(&self, raw: &str) -> Option<&FullTextBackendRegistration> {
        let key = normalize_backend_name(raw);
        self.fulltext_backends.get(&key)
    }

    pub fn operation(&self, command: &str) -> Option<&PluginOperationRegistration> {
        self.operations.get(&normalize_command(command))
    }

    pub fn operations(&self) -> Vec<&PluginOperationRegistration> {
        self.operations.values().collect()
    }

    fn insert_spatial_backend(&mut self, backend: SpatialBackendRegistration) {
        self.spatial_backends
            .insert(normalize_backend_name(backend.name), backend.clone());
        for alias in backend.aliases {
            self.spatial_backends
                .insert(normalize_backend_name(alias), backend.clone());
        }
    }

    fn insert_fulltext_backend(&mut self, backend: FullTextBackendRegistration) {
        self.fulltext_backends
            .insert(normalize_backend_name(backend.name), backend.clone());
        for alias in backend.aliases {
            self.fulltext_backends
                .insert(normalize_backend_name(alias), backend.clone());
        }
    }
}

pub fn builtin_plugin_registry() -> PluginRegistry {
    PluginRegistry::with_builtin_plugins()
}

fn normalize_backend_name(raw: &str) -> String {
    raw.trim().to_ascii_lowercase()
}

fn normalize_command(command: &str) -> String {
    command.trim().to_ascii_uppercase()
}

#[derive(Clone, Debug)]
pub struct CoreSpatialPlugin;

impl RustyRedPlugin for CoreSpatialPlugin {
    fn name(&self) -> &'static str {
        "rustyred.core.spatial"
    }

    fn capabilities(&self) -> Vec<PluginCapability> {
        vec![PluginCapability {
            kind: PluginCapabilityKind::Index,
            name: "spatial".to_string(),
        }]
    }

    fn spatial_backends(&self) -> Vec<SpatialBackendRegistration> {
        vec![
            SpatialBackendRegistration {
                name: SPATIAL_BACKEND_H3,
                aliases: &["", "hand_rolled", "hand-rolled"],
                constructor: |designation| Ok(Box::new(SpatialIndex::for_designation(designation))),
            },
            SpatialBackendRegistration {
                name: SPATIAL_BACKEND_S2,
                aliases: &[],
                constructor: s2_spatial_backend,
            },
        ]
    }
}

#[cfg(feature = "s2")]
fn s2_spatial_backend(
    designation: SpatialDesignation,
) -> Result<Box<dyn SpatialBackend>, SpatialError> {
    Ok(Box::new(crate::spatial_s2::S2SpatialBackend::new(
        designation,
    )))
}

#[cfg(not(feature = "s2"))]
fn s2_spatial_backend(
    _designation: SpatialDesignation,
) -> Result<Box<dyn SpatialBackend>, SpatialError> {
    Err(SpatialError::UnknownBackend(
        "s2 backend requires building with --features s2".to_string(),
    ))
}

#[derive(Clone, Debug)]
pub struct CoreFullTextPlugin;

impl RustyRedPlugin for CoreFullTextPlugin {
    fn name(&self) -> &'static str {
        "rustyred.core.fulltext"
    }

    fn capabilities(&self) -> Vec<PluginCapability> {
        vec![PluginCapability {
            kind: PluginCapabilityKind::Index,
            name: "fulltext".to_string(),
        }]
    }

    fn fulltext_backends(&self) -> Vec<FullTextBackendRegistration> {
        vec![
            FullTextBackendRegistration {
                name: FULLTEXT_BACKEND_HAND_ROLLED,
                aliases: &["", "hand-rolled", "bm25"],
                constructor: |designation| {
                    Ok(Box::new(FullTextIndex::for_designation(designation)))
                },
            },
            FullTextBackendRegistration {
                name: FULLTEXT_BACKEND_TANTIVY,
                aliases: &[],
                constructor: tantivy_fulltext_backend,
            },
        ]
    }
}

#[cfg(feature = "tantivy")]
fn tantivy_fulltext_backend(
    designation: FullTextDesignation,
) -> Result<Box<dyn FullTextBackend>, FullTextBackendError> {
    Ok(Box::new(
        crate::fulltext_tantivy::TantivyFullTextBackend::new(designation)
            .map_err(FullTextBackendError::TantivyInit)?,
    ))
}

#[cfg(not(feature = "tantivy"))]
fn tantivy_fulltext_backend(
    _designation: FullTextDesignation,
) -> Result<Box<dyn FullTextBackend>, FullTextBackendError> {
    Err(FullTextBackendError::UnknownBackend(
        "tantivy backend requires building with --features tantivy".to_string(),
    ))
}

#[derive(Clone, Debug)]
pub struct CoreOperationsPlugin;

impl RustyRedPlugin for CoreOperationsPlugin {
    fn name(&self) -> &'static str {
        "rustyred.core.operations"
    }

    fn capabilities(&self) -> Vec<PluginCapability> {
        vec![PluginCapability {
            kind: PluginCapabilityKind::Operation,
            name: "plugin.echo".to_string(),
        }]
    }

    fn operations(&self) -> Vec<PluginOperationRegistration> {
        vec![PluginOperationRegistration {
            command: "RUSTYRED.PLUGIN.ECHO",
            summary: "Echo a JSON payload through the plugin operation registry.",
            handler: |context, args| {
                RustyredResponse::ok(
                    context.command,
                    "ok",
                    serde_json::json!({
                        "operation": context.command,
                        "args": args,
                    }),
                    context.state_hash,
                )
            },
        }]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct NoopPlugin;

    impl RustyRedPlugin for NoopPlugin {
        fn name(&self) -> &'static str {
            "test.noop"
        }
    }

    #[test]
    fn noop_plugin_registers_and_is_enumerable() {
        let mut registry = PluginRegistry::new();
        registry.register(NoopPlugin);

        let names: Vec<&str> = registry
            .plugins()
            .iter()
            .map(|plugin| plugin.name())
            .collect();
        assert_eq!(names, vec!["test.noop"]);
    }

    #[test]
    fn builtin_registry_enumerates_core_plugins() {
        let registry = builtin_plugin_registry();
        let names: Vec<&str> = registry
            .plugins()
            .iter()
            .map(|plugin| plugin.name())
            .collect();

        assert!(names.contains(&"rustyred.core.spatial"));
        assert!(names.contains(&"rustyred.core.fulltext"));
        assert!(registry.operation("RUSTYRED.PLUGIN.ECHO").is_some());
    }
}
