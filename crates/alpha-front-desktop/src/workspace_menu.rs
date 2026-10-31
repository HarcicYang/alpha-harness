use std::collections::BTreeSet;
use std::path::PathBuf;

use super::*;

impl AlphaDesktop {
    pub(crate) fn toggle_workspace_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.workspace_menu_open {
            self.workspace_menu_open = false;
            self.focus_composer(window, cx);
            cx.notify();
            return;
        }

        self.bump_overlay_generation();
        self.workspace_menu_open = true;
        self.workspace_menu_expanded = Some(self.runtime.active_workspace().to_path_buf());
        self.refresh_workspace_menu(cx);
        cx.notify();
    }

    fn refresh_workspace_menu(&mut self, cx: &mut Context<Self>) {
        match self.runtime.list_sessions() {
            Ok(sessions) => self.workspace_menu_sessions = sessions,
            Err(error) => {
                self.workspace_menu_sessions.clear();
                self.local_status = Some(error.to_string());
            }
        }
        let mut workspaces = BTreeSet::new();
        workspaces.extend(self.runtime.workspaces());
        workspaces.insert(self.runtime.active_workspace().to_path_buf());
        workspaces.extend(self.workspace_menu_sessions.iter().map(session_workspace));
        self.workspace_menu_workspaces = workspaces.into_iter().collect();
        self.workspace_menu_workspaces
            .sort_by_key(|path| workspace_name(&path.display().to_string()).to_lowercase());
        self.reset_scroll(ScrollRegion::WorkspaceList);
        self.reset_scroll(ScrollRegion::WorkspaceSessions);
        cx.notify();
    }

    fn expand_workspace(&mut self, workspace: PathBuf, cx: &mut Context<Self>) {
        if self.workspace_menu_expanded.as_ref() != Some(&workspace) {
            self.workspace_menu_expanded = Some(workspace);
            self.reset_scroll(ScrollRegion::WorkspaceSessions);
            cx.notify();
        }
    }

    fn switch_workspace(
        &mut self,
        workspace: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let changed = self.runtime.active_workspace() != workspace;
        match self.runtime.activate_workspace(&workspace) {
            Ok(()) => {
                self.workspace_menu_expanded = Some(workspace);
                if changed {
                    self.after_workspace_switch(window, cx);
                }
                cx.notify();
                true
            }
            Err(error) => {
                self.local_status = Some(error.to_string());
                cx.notify();
                false
            }
        }
    }

    fn after_workspace_switch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.transcript_epoch = self.transcript_epoch.wrapping_add(1).max(1);
        self.transcript_dirty = true;
        self.command_palette_open = false;
        self.context_open = false;
        self.change_review_open = false;
        self.sessions_open = false;
        self.providers_open = false;
        self.capabilities_open = false;
        self.help_open = false;
        self.composer_menu = None;
        self.expanded_thoughts.clear();
        self.expanded_tool_groups.clear();
        self.tool_outputs.clear();
        self.local_status = None;
        self.reset_scroll(ScrollRegion::Transcript);
        self.composer
            .update(cx, |state, cx| state.set_value("", window, cx));
        self.focus_composer(window, cx);
    }

    fn new_workspace_session(
        &mut self,
        workspace: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.switch_workspace(workspace, window, cx) {
            return;
        }
        if self
            .workspace_menu_expanded
            .as_ref()
            .is_some_and(|path| path == self.runtime.active_workspace())
        {
            self.new_session(cx);
            self.workspace_menu_open = false;
            self.focus_composer(window, cx);
        }
    }

    fn resume_workspace_session(
        &mut self,
        workspace: PathBuf,
        session_id: SessionId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.switch_workspace(workspace, window, cx) {
            return;
        }
        self.resume_session(session_id, cx);
        self.workspace_menu_open = false;
        self.focus_composer(window, cx);
    }

    fn open_workspace_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Open workspace".into()),
        });
        cx.spawn_in(window, async move |this, cx| match receiver.await {
            Ok(Ok(Some(mut paths))) if !paths.is_empty() => {
                let workspace = paths.remove(0);
                let _ = this.update_in(cx, |this, window, cx| {
                    if this.switch_workspace(workspace, window, cx) {
                        this.workspace_menu_open = false;
                        this.focus_composer(window, cx);
                    }
                    cx.notify();
                });
            }
            Ok(Err(error)) => {
                let _ = this.update_in(cx, |this, _, cx| {
                    this.local_status = Some(format!("workspace picker failed: {error}"));
                    cx.notify();
                });
            }
            _ => {}
        })
        .detach();
    }

    pub(crate) fn render_workspace_menu(&mut self, cx: &mut Context<Self>) -> AnyElement {
        if !self.workspace_menu_open {
            return div().into_any_element();
        }

        let active_workspace = self.runtime.active_workspace().to_path_buf();
        let expanded = self
            .workspace_menu_expanded
            .clone()
            .unwrap_or_else(|| active_workspace.clone());
        let expanded_index = self
            .workspace_menu_workspaces
            .iter()
            .position(|workspace| workspace == &expanded)
            .unwrap_or_default();
        let workspace_rows = self
            .workspace_menu_workspaces
            .clone()
            .into_iter()
            .enumerate()
            .map(|(index, workspace)| {
                self.render_workspace_row(
                    index,
                    workspace,
                    active_workspace.clone(),
                    expanded.clone(),
                    cx,
                )
            })
            .collect::<Vec<_>>();
        let workspace_list_height = (workspace_rows.len() as f32 * 42.).clamp(70., 320.);

        let sessions = self
            .workspace_menu_sessions
            .iter()
            .filter(|session| session_workspace(session) == expanded)
            .cloned()
            .collect::<Vec<_>>();
        let session_rows = sessions
            .into_iter()
            .map(|session| {
                let selected = self.runtime.chat().session_id == Some(session.id);
                let preview = session.first_message.as_deref().unwrap_or("Empty session");
                let session_id = session.id;
                let workspace = expanded.clone();
                Button::new(format!("workspace-session:{session_id}"))
                    .child(
                        div()
                            .w_full()
                            .min_h(px(48.))
                            .flex()
                            .flex_col()
                            .items_start()
                            .justify_center()
                            .gap(px(3.))
                            .child(
                                div()
                                    .max_w_full()
                                    .truncate()
                                    .text_size(px(10.))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(rgb(if selected { TEXT } else { TEXT_SECONDARY }))
                                    .child(preview.to_owned()),
                            )
                            .child(
                                div()
                                    .text_size(px(8.))
                                    .font_family("monospace")
                                    .text_color(rgb(if selected { ACCENT } else { FAINT }))
                                    .child(short_id(session_id)),
                            ),
                    )
                    .ghost()
                    .selected(selected)
                    .w_full()
                    .rounded(px(14.))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.resume_workspace_session(workspace.clone(), session_id, window, cx);
                    }))
                    .into_any_element()
            })
            .collect::<Vec<_>>();

        let workspace_name = workspace_name(&expanded.display().to_string());
        let second_layer = div()
            .absolute()
            .top(px(52. + expanded_index as f32 * 44.))
            .left(px(292.))
            .w(px(340.))
            .max_h(px(460.))
            .rounded(px(22.))
            .border_1()
            .border_color(rgba(BORDER))
            .bg(rgb(SURFACE_RAISED))
            .shadow_lg()
            .overflow_hidden()
            .occlude()
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(50.))
                    .flex_none()
                    .px(px(14.))
                    .border_b_1()
                    .border_color(rgba(BORDER_SUBTLE))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .min_w(px(0.))
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(11.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(workspace_name),
                            )
                            .child(
                                div()
                                    .max_w(px(270.))
                                    .truncate()
                                    .text_size(px(8.))
                                    .font_family("monospace")
                                    .text_color(rgb(MUTED))
                                    .child(expanded.display().to_string()),
                            ),
                    )
                    .child(
                        Button::new("workspace-new-session")
                            .label("New")
                            .ghost()
                            .h(px(27.))
                            .rounded_full()
                            .on_click(cx.listener({
                                let workspace = expanded.clone();
                                move |this, _, window, cx| {
                                    this.new_workspace_session(workspace.clone(), window, cx);
                                }
                            })),
                    ),
            )
            .child(
                div()
                    .id("workspace-session-list-shell")
                    .relative()
                    .flex_1()
                    .min_h(px(70.))
                    .smooth_scroll_shell(ScrollRegion::WorkspaceSessions, cx)
                    .child(
                        div()
                            .id("workspace-session-list")
                            .size_full()
                            .overflow_y_scroll()
                            .track_scroll(&self.workspace_session_scroll)
                            .pr(px(SCROLLBAR_GUTTER))
                            .p(px(6.))
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .children(session_rows),
                    )
                    .child(scrollbar_rail(
                        "workspace-sessions".to_owned(),
                        &self.workspace_session_scroll,
                    )),
            );

        let first_layer = div()
            .absolute()
            .top(px(52.))
            .left(px(16.))
            .w(px(272.))
            .rounded(px(22.))
            .border_1()
            .border_color(rgba(BORDER))
            .bg(rgb(SURFACE_RAISED))
            .shadow_lg()
            .overflow_hidden()
            .occlude()
            .p(px(6.))
            .flex()
            .flex_col()
            .gap(px(2.))
            .child(
                div()
                    .h(px(30.))
                    .px(px(8.))
                    .flex()
                    .items_center()
                    .text_size(px(8.5))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(rgb(MUTED))
                    .child("Workspaces"),
            )
            .child(
                div()
                    .id("workspace-list-shell")
                    .relative()
                    .h(px(workspace_list_height))
                    .smooth_scroll_shell(ScrollRegion::WorkspaceList, cx)
                    .child(
                        div()
                            .id("workspace-list")
                            .size_full()
                            .overflow_y_scroll()
                            .track_scroll(&self.workspace_list_scroll)
                            .pr(px(SCROLLBAR_GUTTER))
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .children(workspace_rows),
                    )
                    .child(scrollbar_rail(
                        "workspace-list".to_owned(),
                        &self.workspace_list_scroll,
                    )),
            )
            .child(
                Button::new("open-workspace")
                    .label("Open workspace...")
                    .ghost()
                    .w_full()
                    .h(px(38.))
                    .rounded(px(14.))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_workspace_picker(window, cx);
                    })),
            );

        div()
            .absolute()
            .inset_0()
            .child(div().absolute().inset_0().on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    this.workspace_menu_open = false;
                    this.focus_composer(window, cx);
                    cx.notify();
                }),
            ))
            .child(popover_enter(
                first_layer,
                format!("workspace-menu:{}", self.overlay_generation),
            ))
            .child(popover_enter(
                second_layer,
                format!(
                    "workspace-sessions:{}:{}",
                    expanded.display(),
                    self.overlay_generation
                ),
            ))
            .into_any_element()
    }

    fn render_workspace_row(
        &self,
        index: usize,
        workspace: PathBuf,
        active_workspace: PathBuf,
        expanded: PathBuf,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let active = workspace == active_workspace;
        let is_expanded = workspace == expanded;
        let status = self.runtime.workspace_status(&workspace);
        let running = self.runtime.workspace_running(&workspace);
        let sessions = self
            .workspace_menu_sessions
            .iter()
            .filter(|session| session_workspace(session) == workspace)
            .count();
        let name = workspace_name(&workspace.display().to_string());
        let switch_workspace = workspace.clone();
        let expand_workspace = workspace.clone();
        let hover_workspace = workspace.clone();

        Button::new(("workspace-row", index))
            .child(
                div()
                    .w_full()
                    .h(px(40.))
                    .px(px(8.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        Icon::new(IconName::Folder)
                            .size(px(13.))
                            .text_color(rgb(if active { ACCENT } else { MUTED })),
                    )
                    .child(
                        div()
                            .min_w(px(0.))
                            .flex_1()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(10.))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(rgb(if active { TEXT } else { TEXT_SECONDARY }))
                                    .child(name),
                            )
                            .child(
                                div()
                                    .text_size(px(8.))
                                    .text_color(rgb(FAINT))
                                    .child(format!("{sessions} sessions")),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(5.))
                            .when(running, |this| {
                                this.child(div().size(px(5.)).rounded_full().bg(rgb(ACCENT)))
                            })
                            .when(status == Some(WorkspaceStatus::Disconnected), |this| {
                                this.child(div().size(px(5.)).rounded_full().bg(rgb(DANGER)))
                            })
                            .child(
                                Icon::new(IconName::ChevronRight)
                                    .size(px(10.))
                                    .text_color(rgb(FAINT)),
                            ),
                    ),
            )
            .ghost()
            .selected(is_expanded)
            .w_full()
            .rounded(px(14.))
            .on_hover(cx.listener(move |this, hovered, _, cx| {
                if *hovered {
                    this.expand_workspace(hover_workspace.clone(), cx);
                }
            }))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.expand_workspace(expand_workspace.clone(), cx);
                let _ = this.switch_workspace(switch_workspace.clone(), window, cx);
            }))
            .into_any_element()
    }
}

fn session_workspace(session: &SessionListEntry) -> PathBuf {
    let workspace = PathBuf::from(&session.workspace.root);
    workspace
        .canonicalize()
        .unwrap_or_else(|_| workspace.clone())
}
