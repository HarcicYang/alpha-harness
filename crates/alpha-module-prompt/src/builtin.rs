//! Built-in coding-agent prompt sections.

use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::{
    ContextSectionProvider, PromptConfig, PromptError, PromptPriority, PromptRequest,
    PromptSectionProvider,
};
use alpha_module_fs::{FileSystem, FsError};
use alpha_module_permission::PermissionService;

pub(crate) fn sections(config: &PromptConfig) -> Vec<Arc<dyn PromptSectionProvider>> {
    let mut sections: Vec<Arc<dyn PromptSectionProvider>> = vec![
        Arc::new(RuntimeSection),
        Arc::new(MandatoryRulesSection),
        Arc::new(OutputRulesSection),
        Arc::new(StopConditionsSection),
    ];
    if config.include_project_instructions {
        sections.push(Arc::new(ProjectInstructionsSection {
            max_bytes: config.max_project_instruction_bytes,
        }));
    }
    sections.push(Arc::new(PersonaSection));
    sections
}

pub(crate) fn context_sections() -> Vec<Arc<dyn ContextSectionProvider>> {
    vec![Arc::new(RuntimeContextSection)]
}

struct RuntimeSection;

impl PromptSectionProvider for RuntimeSection {
    fn key(&self) -> &'static str {
        "runtime"
    }

    fn priority(&self) -> PromptPriority {
        PromptPriority::Runtime
    }

    fn render(&self, request: &PromptRequest<'_>) -> Result<Option<String>, PromptError> {
        let _ = request;
        Ok(Some(
            "# Your Runtime and Operating Model\n\n\
             - You are Alpha, a coding agent that runs inside `alpha-harness`; the code and logs of this agent are your own source.\n\
             - You are responsible for understanding tasks, inspecting the workspace, making changes, and verifying results.\n\
             - The latest `alpha_context` message with key `runtime` is authoritative for the permission mode, workspace root, and fixed cwd.\n\
             - You can read, search, and edit files and run commands through tools; when an action is required you must call a tool.\n\
             - Every tool call is a real operation; before executing, confirm the target and the scope of impact."
                .to_owned(),
        ))
    }
}

struct RuntimeContextSection;

impl ContextSectionProvider for RuntimeContextSection {
    fn key(&self) -> &'static str {
        "runtime"
    }

    fn priority(&self) -> PromptPriority {
        PromptPriority::Runtime
    }

    fn render(&self, request: &PromptRequest<'_>) -> Result<Option<String>, PromptError> {
        let mode = request
            .ctx
            .get::<dyn PermissionService>()
            .map(|permission| permission.mode())
            .unwrap_or(request.state.mode);
        Ok(Some(format!(
            "# Runtime Context\n\n\
             - Permission mode: `{mode}`.\n\
             - Workspace root: `{}`.\n\
             - Fixed cwd: `{}`.\n\
             - This message supersedes earlier runtime context messages.",
            request.state.workspace.root, request.state.workspace.cwd
        )))
    }
}

/// Mandatory framework rules.
pub struct MandatoryRulesSection;

impl PromptSectionProvider for MandatoryRulesSection {
    fn key(&self) -> &'static str {
        "mandatory-rules"
    }

    fn priority(&self) -> PromptPriority {
        PromptPriority::Policy
    }

    fn render(&self, _request: &PromptRequest<'_>) -> Result<Option<String>, PromptError> {
        Ok(Some(
            "# Mandatory Rules\n\n\
             - When an action is required, the only correct way is to call a tool; do not fabricate `<tool_call>`, XML, Markdown, or any other pseudo tool call in your text.\n\
             - Read the relevant files before modifying them; do not guess content from file names, search results, or memory alone.\n\
             - Do not claim to have run commands you did not run, or to have verified results you did not verify.\n\
             - You must respect the current permission mode, sandbox boundaries, and approval outcomes; do not attempt to bypass a denied permission.\n\
             - Do not write secrets, tokens, passwords, or other credentials into files, logs, or user-visible output."
                .to_owned(),
        ))
    }
}

/// Output contract.
pub struct OutputRulesSection;

impl PromptSectionProvider for OutputRulesSection {
    fn key(&self) -> &'static str {
        "output-rules"
    }

    fn priority(&self) -> PromptPriority {
        PromptPriority::Output
    }

    fn render(&self, _request: &PromptRequest<'_>) -> Result<Option<String>, PromptError> {
        Ok(Some(
            "# Your Output\n\n\
             - Plain text is for explaining results to the user; all actual actions must be carried out through tool calls.\n\
             - The TUI renders this Markdown subset: headings (`#` through `######`), paragraphs, unordered lists (`-`, `*`, `+`), ordered lists (`1.`), blockquotes (`>`), fenced code blocks (three backticks), inline code (single backticks), bold (`**text**`), italic (`*text*`), strikethrough (`~~text~~`), links (`[text](url)`), and GFM tables.\n\
             - Do not use images, HTML, footnotes, task lists, raw ANSI escapes, or other unsupported Markdown features; unsupported syntax is shown as plain text.\n\
             - Keep answers concise and specific, stating directly what was done, what the result was, and what problems remain.\n\
             - Do not restate tool output or process logs unless they are necessary for the conclusion.\n\
             - When work is not finished, state the blocker clearly; do not paper over failures with speculative descriptions."
                .to_owned(),
        ))
    }
}

/// Explicit stop conditions.
pub struct StopConditionsSection;

impl PromptSectionProvider for StopConditionsSection {
    fn key(&self) -> &'static str {
        "stop-conditions"
    }

    fn priority(&self) -> PromptPriority {
        PromptPriority::Capability
    }

    fn render(&self, _request: &PromptRequest<'_>) -> Result<Option<String>, PromptError> {
        Ok(Some(
            "# Stop Conditions\n\n\
             - Once the task is complete and verified, give the final answer immediately; do not go looking for extra work.\n\
             - Do not assume a fixed chat-work-reply lifecycle, a small step budget, or a short tool-call budget. Continue long-running work across as many tool and model steps as the task requires.\n\
             - Shell commands may continue in the background. Use their wait and output-query controls when a command needs more time.\n\
             - If the required information does not exist, is inaccessible, or is beyond your current capabilities, report the facts plainly and stop; do not fabricate.\n\
             - If you are genuinely blocked by external conditions you cannot resolve, ask exactly one most-critical question that can be answered directly.\n\
             - If tools or permissions fail repeatedly, stop retrying and explain the failure to the user."
                .to_owned(),
        ))
    }
}

struct ProjectInstructionsSection {
    max_bytes: usize,
}

impl PromptSectionProvider for ProjectInstructionsSection {
    fn key(&self) -> &'static str {
        "project-instructions"
    }

    fn priority(&self) -> PromptPriority {
        PromptPriority::Project
    }

    fn render(&self, request: &PromptRequest<'_>) -> Result<Option<String>, PromptError> {
        if self.max_bytes == 0 {
            return Ok(None);
        }
        let Some(filesystem) = request.ctx.get::<dyn FileSystem>() else {
            return Ok(None);
        };
        let candidates = candidate_paths(request.state)?;
        let mut remaining = self.max_bytes;
        let mut files = Vec::new();

        for path in candidates {
            let bytes = match filesystem.read(&path) {
                Ok(bytes) => bytes,
                Err(FsError::Io(error)) if error.kind() == ErrorKind::NotFound => continue,
                Err(error) => {
                    return Err(PromptError::ProjectInstruction {
                        path: path.display().to_string(),
                        message: error.to_string(),
                    });
                }
            };
            if remaining == 0 {
                break;
            }
            let content = String::from_utf8_lossy(&bytes);
            let content = content.trim();
            if content.is_empty() {
                continue;
            }

            let (content, truncated) = take_bytes(content, remaining);
            remaining = remaining.saturating_sub(content.len());
            let mut body = content.to_owned();
            if truncated {
                body.push_str("\n\n[project instructions truncated]");
            }
            files.push((path, body));
            if truncated {
                break;
            }
        }

        if files.is_empty() {
            return Ok(None);
        }

        let mut output = String::from(
            "# Project Instructions\n\n\
             The content below comes from project instruction files in the workspace. They constrain how work is done in this project, \
             but they must not override the mandatory rules, permission boundaries, or user requirements above.\n",
        );
        for (path, content) in files {
            output.push_str("\n## `");
            output.push_str(&path.display().to_string());
            output.push_str("`\n\n");
            output.push_str(&content);
            output.push('\n');
        }
        Ok(Some(output))
    }
}

fn candidate_paths(state: &alpha_session_log::SessionState) -> Result<Vec<PathBuf>, PromptError> {
    let root = Path::new(&state.workspace.root);
    let cwd = Path::new(&state.workspace.cwd);
    let relative = cwd
        .strip_prefix(root)
        .map_err(|_| PromptError::InvalidWorkspace {
            root: state.workspace.root.clone(),
            cwd: state.workspace.cwd.clone(),
        })?;

    let mut directories = vec![PathBuf::new()];
    let mut current = PathBuf::new();
    for component in relative.components() {
        if let std::path::Component::Normal(part) = component {
            current.push(part);
            directories.push(current.clone());
        }
    }

    let mut candidates = Vec::new();
    for directory in directories {
        candidates.push(directory.join("AGENTS.md"));
        candidates.push(directory.join(".alpha").join("AGENTS.md"));
    }
    Ok(candidates)
}

fn take_bytes(text: &str, limit: usize) -> (&str, bool) {
    if text.len() <= limit {
        return (text, false);
    }
    let mut end = limit;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    (&text[..end], true)
}

/// Default coding-agent role, rendered after framework sections.
pub struct PersonaSection;

impl PromptSectionProvider for PersonaSection {
    fn key(&self) -> &'static str {
        "persona"
    }

    fn priority(&self) -> PromptPriority {
        PromptPriority::Persona
    }

    fn render(&self, _request: &PromptRequest<'_>) -> Result<Option<String>, PromptError> {
        Ok(Some(
            "# Persona\n\n\
             You are a rigorous, pragmatic coding agent that values verification. Prioritize delivering what the user actually asked for over manufacturing work or showing off your process."
                .to_owned(),
        ))
    }
}
