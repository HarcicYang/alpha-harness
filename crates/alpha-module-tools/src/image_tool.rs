//! Image inspection tool.

use std::path::Path;
use std::sync::Arc;

use alpha_module_fs::FileSystem;
use alpha_module_permission::{PermissionOutcome, PermissionRequest, PermissionService};
use alpha_schema::{ImageAttachment, PermissionMode};
use base64::Engine as _;
use futures::future::BoxFuture;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::contract::{Tool, ToolContext, ToolError, ToolResponse};

/// Reads an image file and returns it as model-visible image content.
pub struct ViewImageTool {
    filesystem: Arc<dyn FileSystem>,
    permission: Arc<dyn PermissionService>,
}

impl ViewImageTool {
    /// Create an image inspection tool.
    pub fn new(filesystem: Arc<dyn FileSystem>, permission: Arc<dyn PermissionService>) -> Self {
        Self {
            filesystem,
            permission,
        }
    }
}

#[derive(Debug, Deserialize)]
struct ViewImageInput {
    path: String,
}

impl Tool for ViewImageTool {
    fn name(&self) -> &'static str {
        "view_image"
    }

    fn description(&self) -> String {
        "Read a PNG, JPEG, GIF, or WebP file and return the image to the model. \
         Use this when a task requires visual inspection of an image already in the workspace."
            .to_owned()
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" }
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
            let ViewImageInput { path } = serde_json::from_value(input)
                .map_err(|error| ToolError::failure(format!("invalid input: {error}")))?;
            let path_ref = Path::new(&path);
            let mime_type = image_mime_type(path_ref).ok_or_else(|| {
                ToolError::failure(format!(
                    "`{path}` is not a supported image; use PNG, JPEG, GIF, or WebP"
                ))
            })?;
            let sandboxed = self.permission.mode() != PermissionMode::Free;
            let request =
                PermissionRequest::new("fs.read", [path.clone()]).with_sandboxed(sandboxed);
            match self.permission.check(request).await {
                PermissionOutcome::Allowed { .. } => {}
                PermissionOutcome::Denied { reason } => {
                    return Err(ToolError::failure(format!("permission denied: {reason}")));
                }
            }
            let bytes = self
                .filesystem
                .read(path_ref)
                .map_err(|error| ToolError::failure(error.to_string()))?;
            let name = path_ref
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("image")
                .to_owned();
            let byte_count = bytes.len();
            let image = ImageAttachment {
                name: name.clone(),
                mime_type: mime_type.to_owned(),
                data: base64::engine::general_purpose::STANDARD.encode(bytes),
            };
            Ok(ToolResponse::text(format!(
                "Loaded image {name} ({mime_type}, {byte_count} bytes)"
            ))
            .with_images(vec![image])
            .with_output(json!({
                "path": path,
                "mime_type": mime_type,
                "bytes": byte_count,
            })))
        })
    }
}

fn image_mime_type(path: &Path) -> Option<&'static str> {
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        _ => None,
    }
}
