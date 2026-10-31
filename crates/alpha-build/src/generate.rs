//! Cargo dependency graph reading and composition table generation.

use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use cargo_metadata::{Metadata, MetadataCommand, Package, PackageId};
use thiserror::Error;

use crate::check::{BuildError, ModuleInfo, check};
use crate::metadata::{AppMetadata, FrontCommandMetadata, ModuleMetadata, ModuleScope, PackageKind};

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
    /// A module declaration is incomplete.
    #[error("module `{package}` must declare `{field}` in [package.metadata.alpha]")]
    IncompleteModule {
        /// Package name.
        package: String,
        /// Missing field.
        field: &'static str,
    },
    /// Two modules declare the same key.
    #[error("module key `{key}` is declared by both `{first}` and `{second}`")]
    ModuleKeyConflict {
        /// Module key.
        key: String,
        /// First package.
        first: String,
        /// Second package.
        second: String,
    },
    /// Two frontend crates declare the same command.
    #[error("front command `{command}` is declared by both `{first}` and `{second}`")]
    CommandConflict {
        /// Command name.
        command: String,
        /// First package.
        first: String,
        /// Second package.
        second: String,
    },
    /// IO failure while writing generated code.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// One module with everything generated code needs.
#[derive(Clone, Debug)]
struct ModuleSpec {
    info: ModuleInfo,
    scope: ModuleScope,
    module_type: String,
    config_type: String,
}

/// One frontend command.
#[derive(Clone, Debug)]
struct FrontSpec {
    command: String,
    about: String,
    entry: String,
    takes_prompt: bool,
    extern_path: String,
}

/// Read the dependency graph, validate the composition, and generate the
/// composition table into `OUT_DIR`.
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
    let mut modules: Vec<ModuleSpec> = Vec::new();
    let mut contracts: Vec<ModuleInfo> = Vec::new();
    let mut fronts: Vec<FrontSpec> = Vec::new();
    let mut seen: BTreeSet<PackageId> = BTreeSet::new();

    for dep in &app_node.deps {
        let Some(package) = metadata.packages.iter().find(|pkg| pkg.id == dep.pkg) else {
            continue;
        };
        let Some(entry) = module_metadata(package)? else {
            continue;
        };
        match entry.kind {
            PackageKind::Module | PackageKind::Contract => {
                collect_package(
                    package,
                    &entry,
                    dep.name.clone(),
                    &mut modules,
                    &mut contracts,
                    &mut seen,
                )?
            }
            PackageKind::Front => {
                if seen.insert(package.id.clone()) {
                    collect_fronts(package, &entry, dep.name.clone(), &mut fronts)?;
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
                    let Some(inner_entry) = module_metadata(inner)? else {
                        continue;
                    };
                    let extern_path = format!("{}::{}", dep.name, bundle_dep.name);
                    match inner_entry.kind {
                        PackageKind::Module | PackageKind::Contract => collect_package(
                            inner,
                            &inner_entry,
                            extern_path,
                            &mut modules,
                            &mut contracts,
                            &mut seen,
                        )?,
                        PackageKind::Front => {
                            if seen.insert(inner.id.clone()) {
                                collect_fronts(inner, &inner_entry, extern_path, &mut fronts)?;
                            }
                        }
                        PackageKind::Bundle => {}
                    }
                }
            }
        }
    }

    let mut infos: Vec<ModuleInfo> = modules.iter().map(|spec| spec.info.clone()).collect();
    infos.extend(contracts.clone());
    infos.sort_by(|left, right| left.key.cmp(&right.key));
    check(&infos, &overrides)?;
    check_module_keys(&modules)?;
    check_commands(&fronts)?;

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

    let ordered = order_modules(modules);
    fs::write(out_dir.join("alpha_composition.rs"), generate_code(&ordered, &fronts))?;
    Ok(())
}

fn collect_package(
    package: &Package,
    entry: &ModuleMetadata,
    extern_path: String,
    modules: &mut Vec<ModuleSpec>,
    contracts: &mut Vec<ModuleInfo>,
    seen: &mut BTreeSet<PackageId>,
) -> Result<(), GenerateError> {
    if !seen.insert(package.id.clone()) {
        return Ok(());
    }
    let info = to_info(package, entry, extern_path.clone());
    if entry.kind == PackageKind::Contract {
        contracts.push(info);
        return Ok(());
    }
    let scope = entry.scope.ok_or_else(|| GenerateError::IncompleteModule {
        package: package.name.to_string(),
        field: "scope",
    })?;
    let module_type = entry.module.clone().ok_or_else(|| GenerateError::IncompleteModule {
        package: package.name.to_string(),
        field: "module",
    })?;
    let config_type = entry.config.clone().ok_or_else(|| GenerateError::IncompleteModule {
        package: package.name.to_string(),
        field: "config",
    })?;
    modules.push(ModuleSpec {
        info,
        scope,
        module_type: format!("{extern_path}::{module_type}"),
        config_type: format!("{extern_path}::{config_type}"),
    });
    Ok(())
}

fn collect_fronts(
    package: &Package,
    entry: &ModuleMetadata,
    extern_path: String,
    fronts: &mut Vec<FrontSpec>,
) -> Result<(), GenerateError> {
    if entry.commands.is_empty() {
        return Err(GenerateError::IncompleteModule {
            package: package.name.to_string(),
            field: "commands",
        });
    }
    for command in &entry.commands {
        fronts.push(front_spec(command, &extern_path));
    }
    Ok(())
}

fn front_spec(command: &FrontCommandMetadata, extern_path: &str) -> FrontSpec {
    FrontSpec {
        command: command.name.clone(),
        about: command.about.clone(),
        entry: format!("{extern_path}::{}", command.entry),
        takes_prompt: command.takes_prompt,
        extern_path: extern_path.to_owned(),
    }
}

fn check_module_keys(modules: &[ModuleSpec]) -> Result<(), GenerateError> {
    let mut seen: BTreeMap<&str, &str> = BTreeMap::new();
    for spec in modules {
        if let Some(first) = seen.insert(&spec.info.key, &spec.info.package) {
            return Err(GenerateError::ModuleKeyConflict {
                key: spec.info.key.clone(),
                first: first.to_owned(),
                second: spec.info.package.clone(),
            });
        }
    }
    Ok(())
}

fn check_commands(fronts: &[FrontSpec]) -> Result<(), GenerateError> {
    let mut seen: BTreeMap<&str, &str> = BTreeMap::new();
    for front in fronts {
        if let Some(first) = seen.insert(&front.command, &front.extern_path) {
            return Err(GenerateError::CommandConflict {
                command: front.command.clone(),
                first: first.to_owned(),
                second: front.extern_path.clone(),
            });
        }
    }
    Ok(())
}

/// Order modules so a provider activates before a consumer at the same scope.
fn order_modules(mut modules: Vec<ModuleSpec>) -> Vec<ModuleSpec> {
    modules.sort_by(|left, right| left.info.key.cmp(&right.info.key));
    let mut ordered: Vec<ModuleSpec> = Vec::new();
    for scope in [
        ModuleScope::Global,
        ModuleScope::Location,
        ModuleScope::Agent,
    ] {
        let group: Vec<ModuleSpec> = modules
            .iter()
            .filter(|spec| spec.scope == scope)
            .cloned()
            .collect();
        ordered.extend(topological(group));
    }
    ordered
}

fn topological(group: Vec<ModuleSpec>) -> Vec<ModuleSpec> {
    let providers: BTreeMap<String, String> = group
        .iter()
        .flat_map(|spec| {
            spec.info
                .provides
                .iter()
                .map(move |(key, _)| (key.clone(), spec.info.key.clone()))
        })
        .collect();
    let mut remaining: Vec<ModuleSpec> = group;
    let mut ordered: Vec<ModuleSpec> = Vec::new();
    while !remaining.is_empty() {
        let ready = remaining.iter().position(|spec| {
            spec.info.requires.iter().all(|required| {
                let Some(provider) = providers.get(required) else {
                    return true;
                };
                provider == &spec.info.key
                    || ordered.iter().any(|done| &done.info.key == provider)
            })
        });
        match ready {
            Some(index) => ordered.push(remaining.remove(index)),
            None => ordered.append(&mut remaining),
        }
    }
    ordered
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
        requires_config: module.requires_config.clone(),
    }
}

fn generate_code(modules: &[ModuleSpec], fronts: &[FrontSpec]) -> String {
    let mut keys: Vec<&str> = modules.iter().map(|module| module.info.key.as_str()).collect();
    keys.sort_unstable();
    keys.dedup();

    let mut code = String::new();
    code.push_str("// @generated by alpha-build. Do not edit.\n");
    code.push_str("use alpha_shell::host_api::{\n");
    code.push_str(
        "    ActivateError, Ctx, FrontEntry, Host, ModuleEntry, ModuleInput, ModuleScope,\n};\n\n",
    );
    code.push_str("/// Module keys enabled by the Cargo dependency graph.\n");
    code.push_str("pub const ALPHA_MODULE_KEYS: &[&str] = &[\n");
    for key in &keys {
        let _ = writeln!(code, "    {key:?},");
    }
    code.push_str("];\n\n");

    for (index, spec) in modules.iter().enumerate() {
        let needs = spec
            .info
            .requires_config
            .iter()
            .map(|key| format!("{key:?}"))
            .collect::<Vec<_>>()
            .join(", ");
        let _ = writeln!(
            code,
            "fn __activate_{index}(ctx: &Ctx, input: &ModuleInput) -> Result<(), ActivateError> {{"
        );
        if !needs.is_empty() {
            let _ = writeln!(
                code,
                "    if !input.provides_keys(&[{needs}]) {{ return Ok(()); }}"
            );
        }
        if spec.config_type.ends_with("::()") {
            let _ = writeln!(
                code,
                "    {}.activate(ctx, &())",
                spec.module_type
            );
        } else {
            let _ = writeln!(
                code,
                "    let config: {} = input.config({:?})?;",
                spec.config_type, spec.info.key
            );
            let _ = writeln!(code, "    {}.activate(ctx, &config)", spec.module_type);
        }
        code.push_str("}\n\n");
    }

    code.push_str("/// Module activations for this composition.\n");
    code.push_str("pub static ALPHA_MODULES: &[ModuleEntry] = &[\n");
    for (index, spec) in modules.iter().enumerate() {
        let needs = spec
            .info
            .requires_config
            .iter()
            .map(|key| format!("{key:?}"))
            .collect::<Vec<_>>()
            .join(", ");
        let _ = writeln!(
            code,
            "    ModuleEntry {{ key: {:?}, scope: ModuleScope::{:?}, requires_config: &[{}], activate: __activate_{index} }},",
            spec.info.key, spec.scope, needs
        );
    }
    code.push_str("];\n\n");

    code.push_str("/// Front commands for this composition.\n");
    code.push_str("pub static ALPHA_FRONTS: &[FrontEntry] = &[\n");
    for front in fronts {
        let _ = writeln!(
            code,
            "    FrontEntry {{ command: {:?}, about: {:?}, takes_prompt: {}, run: {} }},",
            front.command, front.about, front.takes_prompt, front.entry
        );
    }
    code.push_str("];\n\n");

    code.push_str("/// The composed harness this build produces.\n");
    code.push_str("pub fn host() -> Host {\n    Host { modules: ALPHA_MODULES, fronts: ALPHA_FRONTS }\n}\n");
    code
}
