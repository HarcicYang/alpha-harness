//! Content search tool.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use alpha_module_fs::{FileKind, FileSystem, FsError};
use alpha_module_permission::{PermissionOutcome, PermissionRequest, PermissionService};
use alpha_schema::PermissionMode;
use futures::future::BoxFuture;
use regex::RegexBuilder;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::contract::{Tool, ToolContext, ToolError, ToolResponse};

const MAX_SCANNED_FILES: usize = 10_000;
const MAX_MATCHES: usize = 500;
const IGNORED_DIRS: [&str; 3] = [".git", "target", "node_modules"];

/// Searches file contents under a path.
pub struct GrepTool {
    filesystem: Arc<dyn FileSystem>,
    permission: Arc<dyn PermissionService>,
}

impl GrepTool {
    /// Create a grep tool.
    pub fn new(filesystem: Arc<dyn FileSystem>, permission: Arc<dyn PermissionService>) -> Self {
        Self {
            filesystem,
            permission,
        }
    }
}

#[derive(Debug, Deserialize)]
struct GrepInput {
    pattern: String,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    include: Option<String>,
    #[serde(default)]
    literal: bool,
    #[serde(default = "default_case_sensitive")]
    case_sensitive: bool,
    #[serde(default)]
    limit: Option<usize>,
}

fn default_case_sensitive() -> bool {
    true
}

impl Tool for GrepTool {
    fn name(&self) -> &'static str {
        "grep"
    }

    fn description(&self) -> String {
        "Search file contents under a path with a regular expression and optional include filter."
            .to_owned()
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": { "type": "string" },
                "path": { "type": "string" },
                "include": { "type": "string" },
                "literal": { "type": "boolean" },
                "case_sensitive": { "type": "boolean" },
                "limit": { "type": "integer" }
            },
            "required": ["pattern"]
        })
    }

    fn parallel_safe(&self) -> bool {
        true
    }

    fn execute(
        &self,
        input: Value,
        _context: ToolContext,
    ) -> BoxFuture<'_, Result<ToolResponse, ToolError>> {
        Box::pin(async move {
            let input: GrepInput = serde_json::from_value(input)
                .map_err(|error| ToolError::failure(format!("invalid input: {error}")))?;
            let root = input.path.unwrap_or_else(|| ".".to_owned());
            assert_permission(&self.permission, &root).await?;

            let expression = if input.literal {
                regex::escape(&input.pattern)
            } else {
                input.pattern.clone()
            };
            let regex = RegexBuilder::new(&expression)
                .case_insensitive(!input.case_sensitive)
                .build()
                .map_err(|error| {
                    ToolError::failure(format!("invalid pattern `{}`: {error}", input.pattern))
                })?;
            let limit = input.limit.unwrap_or(100).min(MAX_MATCHES);

            let mut queue = VecDeque::from([PathBuf::from(&root)]);
            let mut matches = Vec::new();
            let mut scanned = 0usize;

            'outer: while let Some(directory) = queue.pop_front() {
                let entries = self.filesystem.list(&directory).map_err(fs_failure)?;
                for entry in entries {
                    if scanned >= MAX_SCANNED_FILES || matches.len() >= limit {
                        break 'outer;
                    }
                    match entry.kind {
                        FileKind::Directory => {
                            if !is_ignored(&entry.path) {
                                queue.push_back(entry.path);
                            }
                        }
                        FileKind::File => {
                            if let Some(include) = &input.include {
                                let name = entry
                                    .path
                                    .file_name()
                                    .map(|name| name.to_string_lossy().into_owned())
                                    .unwrap_or_default();
                                if !include_matches(include, &name) {
                                    continue;
                                }
                            }
                            scanned += 1;
                            let bytes = match self.filesystem.read(&entry.path) {
                                Ok(bytes) => bytes,
                                Err(_) => continue,
                            };
                            let Ok(text) = String::from_utf8(bytes) else {
                                continue;
                            };
                            for (index, line) in text.lines().enumerate() {
                                if regex.is_match(line) {
                                    matches.push(format!(
                                        "{}:{}:{}",
                                        entry.path.display(),
                                        index + 1,
                                        line
                                    ));
                                    if matches.len() >= limit {
                                        break 'outer;
                                    }
                                }
                            }
                        }
                        FileKind::Other => {}
                    }
                }
            }

            let content = if matches.is_empty() {
                "(no matches)".to_owned()
            } else {
                matches.join("\n")
            };
            Ok(ToolResponse::text(content).with_output(json!({
                "matches": matches.len(),
                "scanned_files": scanned,
            })))
        })
    }
}

fn is_ignored(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| IGNORED_DIRS.contains(&name))
}

/// Matches an include filter: glob patterns when `*`/`?` are present,
/// substring matching otherwise.
fn include_matches(pattern: &str, name: &str) -> bool {
    if !pattern.contains('*') && !pattern.contains('?') {
        return name.contains(pattern);
    }
    let mut expression = String::from("^");
    for character in pattern.chars() {
        match character {
            '*' => expression.push_str(".*"),
            '?' => expression.push('.'),
            other => expression.push_str(&regex::escape(&other.to_string())),
        }
    }
    expression.push('$');
    regex::Regex::new(&expression)
        .map(|regex| regex.is_match(name))
        .unwrap_or(false)
}

async fn assert_permission(
    permission: &Arc<dyn PermissionService>,
    resource: &str,
) -> Result<(), ToolError> {
    let sandboxed = permission.mode() != PermissionMode::Free;
    let request =
        PermissionRequest::new("fs.read", [resource.to_owned()]).with_sandboxed(sandboxed);
    match permission.check(request).await {
        PermissionOutcome::Allowed { .. } => Ok(()),
        PermissionOutcome::Denied { reason } => {
            Err(ToolError::failure(format!("permission denied: {reason}")))
        }
    }
}

fn fs_failure(error: FsError) -> ToolError {
    ToolError::failure(error.to_string())
}
