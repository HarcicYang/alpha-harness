//! GPUI desktop frontend for alpha-harness.

#![forbid(unsafe_code)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use alpha_front_runtime::{
    ActivityState, COMMANDS, CommandOutcome, CommandSpec, ContextView, FileChange, FileChangeKind,
    FrontRuntimeManager, InteractionDecision, InteractionPrompt, ModelChoice, TranscriptItem,
    WorkspaceStatus, parse_command, supported_reasoning_levels,
};
use alpha_schema::{
    AssistantContentBlock, CredentialBackendStatus, PermissionMode, ReasoningLevel, SessionId,
    SessionListEntry, ToolCallId, ToolStatus,
};
use gpui_kit::assets::IconName;
use gpui_kit::base::{Disableable as _, Selectable as _};
use gpui_kit::component::{
    Icon, Theme, ThemeMode,
    button::{Button, ButtonVariants as _},
    input::{InputEvent, Textarea, TextareaState},
    scroll::{Scrollbar, ScrollbarMode},
    text::TextView,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

/// Desktop frontend startup failure.
#[derive(Debug, thiserror::Error)]
pub enum DesktopError {
    /// The runtime could not enter the desktop event loop.
    #[error("{0}")]
    Startup(String),
}

struct AlphaDesktop {
    runtime: FrontRuntimeManager,
    focus_handle: FocusHandle,
    composer: Entity<TextareaState>,
    model_search: Entity<TextareaState>,
    model_choices: Vec<ModelChoice>,
    composer_menu: Option<ComposerMenu>,
    command_palette_query: Entity<TextareaState>,
    command_palette_open: bool,
    workspace_menu_open: bool,
    workspace_menu_expanded: Option<PathBuf>,
    workspace_menu_sessions: Vec<SessionListEntry>,
    workspace_menu_workspaces: Vec<PathBuf>,
    submit_requested: bool,
    context_open: bool,
    context_tab: ContextTab,
    change_review_open: bool,
    selected_change: usize,
    thinking_expanded: bool,
    expanded_tool_group: Option<String>,
    sessions_open: bool,
    session_search: Entity<TextareaState>,
    session_selected: Option<SessionId>,
    session_delete_confirm: Option<SessionId>,
    session_status: Option<String>,
    sessions: Vec<SessionListEntry>,
    providers_open: bool,
    provider_kind: Option<String>,
    provider_selected: Option<String>,
    provider_remove_confirm: Option<String>,
    provider_status: Option<String>,
    provider_credential: Entity<TextareaState>,
    provider_config: Entity<TextareaState>,
    capabilities_open: bool,
    capabilities_text: Entity<TextareaState>,
    overlay_generation: u64,
    slash_palette_open: bool,
    transcript_epoch: u64,
    context_tab_generation: u64,
    model_scroll: ScrollHandle,
    session_scroll: ScrollHandle,
    provider_kind_scroll: ScrollHandle,
    provider_scroll: ScrollHandle,
    context_scroll: ScrollHandle,
    command_scroll: ScrollHandle,
    slash_scroll: ScrollHandle,
    change_file_scroll: ScrollHandle,
    change_diff_scroll: ScrollHandle,
    tool_scroll: ScrollHandle,
    composer_menu_scroll: ScrollHandle,
    workspace_list_scroll: ScrollHandle,
    workspace_session_scroll: ScrollHandle,
    transcript_scroll: ScrollHandle,
    scroll_motion: [SmoothScroll; SCROLL_REGION_COUNT],
    transcript_items: Arc<Vec<DesktopTranscriptItem>>,
    transcript_dirty: bool,
    jump_to_latest_hidden_until: Option<Instant>,
    help_open: bool,
    local_status: Option<String>,
    _poll_task: Option<Task<()>>,
    _composer_subscription: Subscription,
    _model_search_subscription: Subscription,
    _command_palette_subscription: Subscription,
    _session_search_subscription: Subscription,
    _capabilities_subscription: Subscription,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ComposerMenu {
    Mode,
    Model,
    Effort,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ContextTab {
    Changes,
    Activity,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum DesktopTranscriptItem {
    User(SharedString),
    Assistant(SharedString),
    StreamingAssistant(SharedString),
    Thinking(SharedString),
    StreamingThinking(SharedString),
    Tools { id: String, tools: Vec<ToolSummary> },
    Error(SharedString),
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ToolSummary {
    id: ToolCallId,
    name: String,
    status: Option<ToolStatus>,
    content: Option<String>,
}

mod actions;
mod capabilities;
mod change_review;
mod command_palette;
mod composer;
mod context_panel;
mod desktop_commands;
mod helpers;
mod model;
mod model_picker;
mod overlays;
mod permission;
mod provider_actions;
mod provider_content;
mod provider_rows;
mod providers;
mod scroll;
mod session_actions;
mod session_summary;
mod sessions;
mod shell;
mod slash_palette;
mod theme;
mod transcript;
mod ui;
mod workspace_menu;

use desktop_commands::*;
use helpers::*;
use model::*;
use session_summary::*;
use theme::*;
use ui::*;

/// Run the desktop application until its window closes.
pub fn run_desktop(runtime: FrontRuntimeManager) -> Result<(), DesktopError> {
    let mut runtime = Some(runtime);
    gpui_kit::application()
        .with_assets(gpui_kit::assets::AllAssets)
        .run(move |cx| {
            gpui_kit::init(cx);
            apply_theme(cx);
            let runtime = runtime
                .take()
                .ok_or_else(|| DesktopError::Startup("desktop runtime consumed".to_owned()))
                .expect("desktop runtime is initialized once");
            gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::centered(size(px(1280.), px(800.)), cx)),
                    window_min_size: Some(size(px(560.), px(560.))),
                    inactive_frame_interval: Some(Duration::from_millis(8)),
                    ..WindowOptions::default()
                },
                cx,
                move |window, cx| cx.new(|cx| AlphaDesktop::new(runtime, window, cx)),
            )
            .expect("desktop window opens");
        });
    Ok(())
}

#[cfg(test)]
mod tests {
    use alpha_front_runtime::{ContextView, TranscriptItem};
    use alpha_schema::{
        PermissionMode, SessionId, SessionListEntry, Timestamp, ToolCallId, ToolStatus,
        WorkspaceRef,
    };

    use super::{
        DesktopTranscriptItem, command_suggestions, context_capabilities, desktop_transcript_items,
        format_token_count, parse_provider_config, path_parts, pretty_provider_config,
        session_matches_query, workspace_name,
    };

    #[test]
    fn consecutive_tool_items_render_as_one_group_with_the_latest_tool() {
        let first = ToolCallId::new();
        let second = ToolCallId::new();
        let items = vec![
            TranscriptItem::ToolCall {
                id: first,
                name: "read".to_owned(),
                input: serde_json::json!({ "path": "README.md" }),
            },
            TranscriptItem::ToolResult {
                id: first,
                name: "read".to_owned(),
                status: ToolStatus::Completed,
                content: "README".to_owned(),
            },
            TranscriptItem::ToolCall {
                id: second,
                name: "grep".to_owned(),
                input: serde_json::json!({ "pattern": "desktop" }),
            },
            TranscriptItem::ToolResult {
                id: second,
                name: "grep".to_owned(),
                status: ToolStatus::Completed,
                content: "4 matches".to_owned(),
            },
        ];

        let rendered = desktop_transcript_items(&items);
        assert_eq!(rendered.len(), 1);
        let DesktopTranscriptItem::Tools { id, tools } = &rendered[0] else {
            panic!("consecutive tools must render as a tool group");
        };
        assert_eq!(id, &first.to_string());
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[1].name, "grep");
        assert_eq!(tools[1].status, Some(ToolStatus::Completed));
        assert_eq!(tools[1].content.as_deref(), Some("4 matches"));
    }

    #[test]
    fn durable_assistant_thinking_is_preserved_before_the_response() {
        let rendered = desktop_transcript_items(&[TranscriptItem::AssistantMessage {
            blocks: vec![
                alpha_schema::AssistantContentBlock::Thinking {
                    text: "inspect the workspace".to_owned(),
                    signature: None,
                },
                alpha_schema::AssistantContentBlock::Text {
                    text: "The workspace is ready.".to_owned(),
                },
            ],
        }]);
        assert_eq!(
            rendered,
            vec![
                DesktopTranscriptItem::Thinking("inspect the workspace".into()),
                DesktopTranscriptItem::Assistant("The workspace is ready.".into()),
            ]
        );
    }

    #[test]
    fn live_text_keeps_a_streaming_presentation_variant() {
        let rendered = desktop_transcript_items(&[
            TranscriptItem::AssistantDelta {
                text: "streaming".to_owned(),
            },
            TranscriptItem::ThinkingDelta {
                text: "reasoning".to_owned(),
            },
        ]);
        assert_eq!(
            rendered,
            vec![
                DesktopTranscriptItem::StreamingAssistant("streaming".into()),
                DesktopTranscriptItem::StreamingThinking("reasoning".into()),
            ]
        );
    }

    #[test]
    fn context_helpers_format_paths_tokens_and_capabilities() {
        assert_eq!(
            path_parts("crates/alpha-front-desktop/src/lib.rs"),
            (
                "lib.rs".to_owned(),
                "crates/alpha-front-desktop/src".to_owned()
            )
        );
        assert_eq!(
            path_parts("README.md"),
            ("README.md".to_owned(), "Workspace root".to_owned())
        );
        assert_eq!(format_token_count(987), "987");
        assert_eq!(format_token_count(12_400), "12.4k");
        assert_eq!(format_token_count(1_250_000), "1.2m");
        assert_eq!(
            workspace_name("/run/media/user/projects/alpha-harness"),
            "alpha-harness"
        );
        assert_eq!(workspace_name(""), "workspace");

        let context = ContextView {
            local_tools: vec!["read".to_owned(), "edit".to_owned()],
            provider_tools: vec!["edit".to_owned(), "web_search".to_owned()],
            ..ContextView::default()
        };
        assert_eq!(
            context_capabilities(&context),
            [
                "read".to_owned(),
                "edit".to_owned(),
                "web_search".to_owned()
            ]
        );
    }

    #[test]
    fn provider_config_round_trips_and_rejects_non_objects() {
        let config = serde_json::json!({
            "base_url": "https://example.test",
            "headers": { "x-test": "1" }
        });
        let formatted = pretty_provider_config(&config);
        assert_eq!(parse_provider_config(&formatted).unwrap(), config);
        assert_eq!(
            parse_provider_config("[]").unwrap_err(),
            "configuration must be a JSON object"
        );
        assert!(
            parse_provider_config("{")
                .unwrap_err()
                .starts_with("invalid JSON:")
        );
    }

    #[test]
    fn session_search_matches_preview_route_and_workspace() {
        let session = SessionListEntry {
            id: SessionId::new(),
            created_at: Timestamp::from_millis(1),
            mode: PermissionMode::Auto,
            workspace: WorkspaceRef {
                root: "/workspace/alpha".to_owned(),
                cwd: "/workspace/alpha".to_owned(),
            },
            provider: "deepseek".to_owned(),
            model: "chat".to_owned(),
            first_message: Some("Fix the desktop session manager".to_owned()),
        };
        assert!(session_matches_query(&session, "DESKTOP"));
        assert!(session_matches_query(&session, "deepseek/chat"));
        assert!(session_matches_query(&session, "workspace/alpha"));
        assert!(!session_matches_query(&session, "unrelated"));
    }

    #[test]
    fn slash_command_suggestions_filter_by_name() {
        let commands = command_suggestions("mo");
        let names = commands
            .iter()
            .map(|command| command.name)
            .collect::<Vec<_>>();
        assert_eq!(names, ["model", "mode"]);
        assert_eq!(
            command_suggestions("").len(),
            alpha_front_runtime::COMMANDS.len()
        );
        assert!(command_suggestions("missing").is_empty());
    }
}
