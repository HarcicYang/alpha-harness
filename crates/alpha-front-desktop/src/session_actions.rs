use super::*;

impl AlphaDesktop {
    pub(crate) fn open_sessions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.bump_overlay_generation();
        self.reset_scroll(ScrollRegion::Sessions);
        match self.runtime.list_sessions() {
            Ok(sessions) => {
                let active = self.runtime.chat().session_id;
                self.session_selected = active
                    .filter(|id| sessions.iter().any(|session| session.id == *id))
                    .or_else(|| sessions.first().map(|session| session.id));
                self.sessions = sessions;
                self.session_delete_confirm = None;
                self.session_status = None;
                self.session_search
                    .update(cx, |state, cx| state.set_value("", window, cx));
                let focus = self.session_search.read(cx).focus_handle(cx);
                window.focus(&focus, cx);
                self.sessions_open = true;
                self.local_status = None;
            }
            Err(error) => self.local_status = Some(error.to_string()),
        }
        cx.notify();
    }

    pub(crate) fn resume_session(&mut self, session_id: SessionId, cx: &mut Context<Self>) {
        match self.runtime.resume_session(session_id) {
            Ok(_) => {
                self.sessions_open = false;
                self.session_delete_confirm = None;
                self.session_status = None;
                self.change_review_open = false;
                self.selected_change = 0;
                self.context_tab = ContextTab::Changes;
                self.thinking_expanded = false;
                self.expanded_tool_group = None;
                self.jump_to_latest_hidden_until = None;
                self.scroll_to_bottom(ScrollRegion::Transcript);
                self.local_status = None;
            }
            Err(error) => self.local_status = Some(error.to_string()),
        }
        cx.notify();
    }

    pub(crate) fn new_session(&mut self, cx: &mut Context<Self>) {
        self.draft_epoch = self.draft_epoch.wrapping_add(1).max(1);
        self.runtime.new_session();
        self.sessions_open = false;
        self.session_delete_confirm = None;
        self.session_status = None;
        self.composer_menu = None;
        self.change_review_open = false;
        self.selected_change = 0;
        self.context_tab = ContextTab::Changes;
        self.thinking_expanded = false;
        self.expanded_tool_group = None;
        self.jump_to_latest_hidden_until = None;
        self.scroll_to_bottom(ScrollRegion::Transcript);
        self.local_status = None;
        cx.notify();
    }

    pub(crate) fn delete_selected_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session_id) = self.session_selected else {
            return;
        };
        if self.session_delete_confirm != Some(session_id) {
            self.session_delete_confirm = Some(session_id);
            self.session_status = Some("delete confirmation required".to_owned());
            cx.notify();
            return;
        }
        match self.runtime.delete_session(session_id) {
            Ok(()) => {
                self.session_status = Some(format!("deleted {}", short_id(session_id)));
                match self.runtime.list_sessions() {
                    Ok(sessions) => {
                        self.sessions = sessions;
                        self.session_selected = self.sessions.first().map(|session| session.id);
                    }
                    Err(error) => self.session_status = Some(error.to_string()),
                }
                self.session_delete_confirm = None;
                self.session_search
                    .update(cx, |state, cx| state.set_value("", window, cx));
            }
            Err(error) => self.session_status = Some(error.to_string()),
        }
        cx.notify();
    }
}
