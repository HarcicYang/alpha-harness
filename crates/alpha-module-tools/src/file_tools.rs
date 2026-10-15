//! File tools: read, write, and edit.

use std::path::Path;
use std::sync::Arc;

use alpha_module_fs::{FileSystem, FsError};
use alpha_module_permission::{PermissionOutcome, PermissionRequest, PermissionService};
use futures::future::BoxFuture;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::contract::{Tool, ToolContext, ToolError, ToolResponse, truncate_head_tail};

const MAX_CONTENT_BYTES: usize = 64 * 1024;
const DEFAULT_READ_LINES: usize = 2000;

/// Reads text files inside the workspace.
pub struct ReadTool {
    filesystem: Arc<dyn FileSystem>,
    permission: Arc<dyn PermissionService>,
}

impl ReadTool {
    /// Create a read tool.
    pub fn new(filesystem: Arc<dyn FileSystem>, permission: Arc<dyn PermissionService>) -> Self {
        Self {
            filesystem,
            permission,
        }
    }
}

#[derive(Debug, Deserialize)]
struct ReadInput {
    path: String,
    #[serde(default)]
    offset: Option<usize>,
    #[serde(default)]
    limit: Option<usize>,
}

impl Tool for ReadTool {
    fn name(&self) -> &'static str {
        "read"
    }

    fn description(&self) -> String {
        "Read a text file inside the workspace, with optional line offset and limit.".to_owned()
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" },
                "offset": { "type": "integer" },
                "limit": { "type": "integer" }
            },
            "required": ["path"]
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
            let input: ReadInput = parse_input(input)?;
            assert_permission(&self.permission, "fs.read", &input.path).await?;
            let bytes = self
                .filesystem
                .read(Path::new(&input.path))
                .map_err(fs_failure)?;
            let text = String::from_utf8(bytes)
                .map_err(|_| ToolError::failure(format!("`{}` is not valid UTF-8", input.path)))?;

            let lines: Vec<&str> = text.lines().collect();
            let offset = input.offset.unwrap_or(0);
            let limit = input.limit.unwrap_or(DEFAULT_READ_LINES);
            let selected: Vec<&str> = lines.iter().skip(offset).take(limit).copied().collect();
            let joined = if text.is_empty() {
                "(empty file)".to_owned()
            } else {
                selected.join("\n")
            };
            let (content, bytes_truncated) = truncate_head_tail(&joined, MAX_CONTENT_BYTES);
            let truncated = bytes_truncated || offset + selected.len() < lines.len();

            Ok(ToolResponse::text(content).with_output(json!({
                "bytes": text.len(),
                "lines": lines.len(),
                "truncated": truncated,
            })))
        })
    }
}

/// Writes whole files atomically.
pub struct WriteTool {
    filesystem: Arc<dyn FileSystem>,
    permission: Arc<dyn PermissionService>,
}

impl WriteTool {
    /// Create a write tool.
    pub fn new(filesystem: Arc<dyn FileSystem>, permission: Arc<dyn PermissionService>) -> Self {
        Self {
            filesystem,
            permission,
        }
    }
}

#[derive(Debug, Deserialize)]
struct WriteInput {
    path: String,
    content: String,
}

impl Tool for WriteTool {
    fn name(&self) -> &'static str {
        "write"
    }

    fn description(&self) -> String {
        "Write a file inside the workspace, replacing existing content atomically.".to_owned()
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" },
                "content": { "type": "string" }
            },
            "required": ["path", "content"]
        })
    }

    fn execute(
        &self,
        input: Value,
        _context: ToolContext,
    ) -> BoxFuture<'_, Result<ToolResponse, ToolError>> {
        Box::pin(async move {
            let input: WriteInput = parse_input(input)?;
            assert_permission(&self.permission, "fs.write", &input.path).await?;
            self.filesystem
                .write(Path::new(&input.path), input.content.as_bytes())
                .map_err(fs_failure)?;
            Ok(ToolResponse::text(format!(
                "Wrote {} bytes to {}",
                input.content.len(),
                input.path
            ))
            .with_output(json!({ "bytes_written": input.content.len() })))
        })
    }
}

/// Replaces text in a file.
pub struct EditTool {
    filesystem: Arc<dyn FileSystem>,
    permission: Arc<dyn PermissionService>,
}

impl EditTool {
    /// Create an edit tool.
    pub fn new(filesystem: Arc<dyn FileSystem>, permission: Arc<dyn PermissionService>) -> Self {
        Self {
            filesystem,
            permission,
        }
    }
}

#[derive(Debug, Deserialize)]
struct EditInput {
    path: String,
    old: String,
    new: String,
    #[serde(default)]
    replace_all: bool,
}

impl Tool for EditTool {
    fn name(&self) -> &'static str {
        "edit"
    }

    fn description(&self) -> String {
        "Replace text in a file. By default the old text must appear exactly once.".to_owned()
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" },
                "old": { "type": "string" },
                "new": { "type": "string" },
                "replace_all": { "type": "boolean" }
            },
            "required": ["path", "old", "new"]
        })
    }

    fn execute(
        &self,
        input: Value,
        _context: ToolContext,
    ) -> BoxFuture<'_, Result<ToolResponse, ToolError>> {
        Box::pin(async move {
            let input: EditInput = parse_input(input)?;
            if input.old.is_empty() {
                return Err(ToolError::failure("old text must not be empty"));
            }
            assert_permission(&self.permission, "fs.read", &input.path).await?;
            let bytes = self
                .filesystem
                .read(Path::new(&input.path))
                .map_err(fs_failure)?;
            let text = String::from_utf8(bytes)
                .map_err(|_| ToolError::failure(format!("`{}` is not valid UTF-8", input.path)))?;
            let matches = text.matches(&input.old).count();
            if matches == 0 {
                return Err(ToolError::failure(format!(
                    "old text not found in {}",
                    input.path
                )));
            }
            if matches > 1 && !input.replace_all {
                return Err(ToolError::failure(format!(
                    "old text appears {matches} times in {}; set replace_all to true",
                    input.path
                )));
            }

            assert_permission(&self.permission, "fs.write", &input.path).await?;
            let replacements = if input.replace_all { matches } else { 1 };
            let updated = if input.replace_all {
                text.replace(&input.old, &input.new)
            } else {
                text.replacen(&input.old, &input.new, 1)
            };
            self.filesystem
                .write(Path::new(&input.path), updated.as_bytes())
                .map_err(fs_failure)?;
            Ok(ToolResponse::text(format!(
                "Replaced {replacements} occurrence(s) in {}",
                input.path
            ))
            .with_output(json!({ "replacements": replacements })))
        })
    }
}

fn parse_input<T: DeserializeOwned>(input: Value) -> Result<T, ToolError> {
    serde_json::from_value(input)
        .map_err(|error| ToolError::failure(format!("invalid input: {error}")))
}

async fn assert_permission(
    permission: &Arc<dyn PermissionService>,
    action: &str,
    resource: &str,
) -> Result<(), ToolError> {
    let request = PermissionRequest::new(action, [resource.to_owned()]).with_sandboxed(true);
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
