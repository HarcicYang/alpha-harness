//! Cargo dependency graph reading and module table generation.

use std::collections::BTreeMap;
use std::env;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use cargo_metadata::{Metadata, MetadataCommand, Package, PackageId};
use thiserror::Error;

use crate::check::{BuildError, ModuleInfo, check};
use crate::metadata::{AppMetadata, ModuleMetadata, PackageKind};

/// Errors from the build-time generation step.
#[derive(Debug, Error)]
pub enum GenerateError {
    /// Composition validation failed.
    #[error(transparent)]
    Build(#[from] BuildError),
    /// `cargo metadata` failed.
    #[error("failed to read cargo metadata: {0}")]
    Metadata(#[from] cargo_metadata::Error),
    /// Metadata JSON did not match the expected shape.
    #[error("invalid [package.metadata.alpha] in `{package}`: {source}")]
    PackageMetadata {
        /// Package name.
        package: String,
        /// JSON error.
        #[source]
        source: serde_json::Error,
    },
    /// Required environment variables are missing.
    #[error("missing environment variable `{0}`; run this from a build script")]
    MissingEnv(&'static str),
    /// IO failure while writing generated code.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Read the dependency graph, validate the composition, and generate the
/// module table into `OUT_DIR`.
pub fn check_and_generate() -> Result<(), GenerateError> {
    let manifest_dir = PathBuf::from(
        env::var("CARGO_MANIFEST_DIR")
            .map_err(|_| GenerateError::MissingEnv("CARGO_MANIFEST_DIR"))?,
    );
    let out_dir =
        PathBuf::from(env::var("OUT_DIR").map_err(|_| GenerateError::MissingEnv("OUT_DIR"))?);

    let metadata = MetadataCommand::new()
        .manifest_path(manifest_dir.join("Cargo.toml"))
        .exec()?;
    let resolve = metadata.resolve.as_ref().ok_or(GenerateError::Metadata(
        cargo_metadata::Error::CargoMetadata {
            stderr: "cargo metadata returned no resolve graph".to_owned(),
        },
    ))?;

    let app = find_app(&metadata, &manifest_dir)?;
    let app_node = resolve
        .nodes
        .iter()
        .find(|node| node.id == app.id)
        .ok_or_else(|| {
            GenerateError::Metadata(cargo_metadata::Error::CargoMetadata {
                stderr: format!("no resolve node for `{}`", app.name),
            })
        })?;

    let overrides = app_metadata(app)?;
    let mut modules: Vec<ModuleInfo> = Vec::new();
    let mut seen: BTreeMap<PackageId, ()> = BTreeMap::new();

    for dep in &app_node.deps {
        let Some(package) = metadata.packages.iter().find(|pkg| pkg.id == dep.pkg) else {
            continue;
        };
        let Some(module) = module_metadata(package)? else {
            continue;
        };
        match module.kind {
            PackageKind::Module => {
                if seen.insert(package.id.clone(), ()).is_none() {
                    modules.push(to_info(package, &module, dep.name.clone()));
                }
            }
            PackageKind::Bundle => {
                let Some(bundle_node) = resolve.nodes.iter().find(|node| node.id == package.id)
                else {
                    continue;
                };
                for bundle_dep in &bundle_node.deps {
                    let Some(inner) = metadata
                        .packages
                        .iter()
                        .find(|pkg| pkg.id == bundle_dep.pkg)
                    else {
                        continue;
                    };
                    let Some(inner_module) = module_metadata(inner)? else {
                        continue;
                    };
                    if inner_module.kind != PackageKind::Module {
                        continue;
                    }
                    if seen.insert(inner.id.clone(), ()).is_none() {
                        modules.push(to_info(
                            inner,
                            &inner_module,
                            format!("{}::{}", dep.name, bundle_dep.name),
                        ));
                    }
                }
            }
        }
    }

    modules.sort_by(|left, right| left.key.cmp(&right.key));
    check(&modules, &overrides)?;

    for package in metadata
        .packages
        .iter()
        .filter(|pkg| pkg.metadata.get("alpha").is_some())
    {
        println!("cargo:rerun-if-changed={}", package.manifest_path);
    }
    let lock = metadata.workspace_root.join("Cargo.lock");
    if lock.exists() {
        println!("cargo:rerun-if-changed={lock}");
    }

    fs::write(out_dir.join("alpha_modules.rs"), generate_code(&modules))?;
    Ok(())
}

fn find_app<'a>(metadata: &'a Metadata, manifest_dir: &Path) -> Result<&'a Package, GenerateError> {
    metadata
        .packages
        .iter()
        .find(|package| package.manifest_path.as_std_path().parent() == Some(manifest_dir))
        .ok_or_else(|| {
            GenerateError::Metadata(cargo_metadata::Error::CargoMetadata {
                stderr: format!("no package for manifest dir {}", manifest_dir.display()),
            })
        })
}

fn app_metadata(app: &Package) -> Result<Vec<String>, GenerateError> {
    let Some(value) = app.metadata.get("alpha") else {
        return Ok(Vec::new());
    };
    let parsed: AppMetadata =
        serde_json::from_value(value.clone()).map_err(|source| GenerateError::PackageMetadata {
            package: app.name.to_string(),
            source,
        })?;
    Ok(parsed.overrides)
}

fn module_metadata(package: &Package) -> Result<Option<ModuleMetadata>, GenerateError> {
    let Some(value) = package.metadata.get("alpha") else {
        return Ok(None);
    };
    let parsed: ModuleMetadata =
        serde_json::from_value(value.clone()).map_err(|source| GenerateError::PackageMetadata {
            package: package.name.to_string(),
            source,
        })?;
    Ok(Some(parsed))
}

fn to_info(package: &Package, module: &ModuleMetadata, extern_path: String) -> ModuleInfo {
    ModuleInfo {
        package: package.name.to_string(),
        key: module.key.clone(),
        extern_path,
        api: module.api,
        provides: module
            .provides
            .iter()
            .map(|capability| (capability.key.clone(), capability.kind))
            .collect(),
        requires: module.requires.clone(),
        registry_entries: module.registry_entries.clone(),
    }
}

fn generate_code(modules: &[ModuleInfo]) -> String {
    let mut keys: Vec<&str> = modules.iter().map(|module| module.key.as_str()).collect();
    keys.sort_unstable();
    keys.dedup();

    let mut code = String::new();
    code.push_str("// @generated by alpha-build. Do not edit.\n");
    code.push_str("/// Module keys enabled by the Cargo dependency graph.\n");
    code.push_str("pub const ALPHA_MODULE_KEYS: &[&str] = &[\n");
    for key in keys {
        let _ = writeln!(code, "    {key:?},");
    }
    code.push_str("];\n\n");
    code.push_str("#[allow(unused)]\nconst _ALPHA_LINKAGE: () = {\n");
    for module in modules {
        let _ = writeln!(
            code,
            "    let _ = {}::__ALPHA_MODULE__;",
            module.extern_path
        );
    }
    code.push_str("};\n");
    code
}
