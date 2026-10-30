use std::collections::BTreeMap;
use std::ops::{Deref, DerefMut};
use std::path::{Path, PathBuf};

use super::{FrontError, FrontEvent, FrontRuntime, FrontRuntimeConfig};

/// Lifecycle state of one managed workspace core.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WorkspaceStatus {
    /// The core is connected and can receive requests.
    #[default]
    Running,
    /// The core disconnected after its restart policy was exhausted.
    Disconnected,
}

/// Event generated while a non-active workspace core is running.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceNotice {
    /// Workspace that produced the event.
    pub workspace: PathBuf,
    /// User-facing event text.
    pub message: String,
}

/// Result of polling every managed workspace.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WorkspacePoll {
    /// The active runtime produced state changes.
    pub active_changed: bool,
    /// At least one runtime produced state changes.
    pub any_changed: bool,
}

struct ManagedRuntime {
    runtime: FrontRuntime,
    status: WorkspaceStatus,
}

/// Owns the desktop's primary core and every background workspace core.
pub struct FrontRuntimeManager {
    base_config: FrontRuntimeConfig,
    active_workspace: PathBuf,
    runtimes: BTreeMap<PathBuf, ManagedRuntime>,
    notices: Vec<WorkspaceNotice>,
}

impl FrontRuntimeManager {
    /// Start the primary runtime for the configured workspace.
    pub fn start(mut config: FrontRuntimeConfig) -> Result<Self, FrontError> {
        let active_workspace = normalize_workspace(&config.session.workspace);
        config.session.workspace = active_workspace.clone();
        let runtime = FrontRuntime::start(config.clone())?;
        let mut runtimes = BTreeMap::new();
        runtimes.insert(
            active_workspace.clone(),
            ManagedRuntime {
                runtime,
                status: WorkspaceStatus::Running,
            },
        );
        Ok(Self {
            base_config: config,
            active_workspace,
            runtimes,
            notices: Vec::new(),
        })
    }

    /// Active workspace root.
    pub fn active_workspace(&self) -> &Path {
        &self.active_workspace
    }

    /// Roots for every workspace with a running managed core.
    pub fn workspaces(&self) -> Vec<PathBuf> {
        self.runtimes.keys().cloned().collect()
    }

    /// Current core state for one workspace.
    pub fn workspace_status(&self, workspace: &Path) -> Option<WorkspaceStatus> {
        self.runtimes
            .get(&normalize_workspace(workspace))
            .map(|managed| managed.status)
    }

    /// Whether a workspace has an active model turn.
    pub fn workspace_running(&self, workspace: &Path) -> bool {
        self.runtimes
            .get(&normalize_workspace(workspace))
            .is_some_and(|managed| managed.runtime.chat().running_turn.is_some())
    }

    /// Switch the primary core, starting one when the workspace is new.
    pub fn activate_workspace(&mut self, workspace: impl AsRef<Path>) -> Result<(), FrontError> {
        let workspace = normalize_workspace(workspace.as_ref());
        if workspace == self.active_workspace {
            return Ok(());
        }

        let needs_restart = self
            .runtimes
            .get(&workspace)
            .is_some_and(|managed| managed.status == WorkspaceStatus::Disconnected);
        if needs_restart {
            self.runtimes.remove(&workspace);
        }
        if !self.runtimes.contains_key(&workspace) {
            let mut config = self.base_config.clone();
            config.session.workspace = workspace.clone();
            let runtime = FrontRuntime::start(config)?;
            self.runtimes.insert(
                workspace.clone(),
                ManagedRuntime {
                    runtime,
                    status: WorkspaceStatus::Running,
                },
            );
        }
        self.active_workspace = workspace;
        Ok(())
    }

    /// Poll every managed core and retain background notifications.
    pub fn poll_workspaces(&mut self) -> Result<WorkspacePoll, FrontError> {
        let active_workspace = self.active_workspace.clone();
        let mut poll = WorkspacePoll::default();
        let mut error = None;

        for (workspace, managed) in &mut self.runtimes {
            if managed.status == WorkspaceStatus::Disconnected {
                continue;
            }
            let events = match managed.runtime.try_receive_events() {
                Ok(events) => events,
                Err(failure) => {
                    managed.status = WorkspaceStatus::Disconnected;
                    self.notices.push(WorkspaceNotice {
                        workspace: workspace.clone(),
                        message: format!("core stopped: {failure}"),
                    });
                    if *workspace == active_workspace {
                        error = Some(failure);
                    }
                    continue;
                }
            };
            if events.is_empty() {
                continue;
            }

            poll.any_changed = true;
            poll.active_changed |= *workspace == active_workspace;
            for event in events {
                match event {
                    FrontEvent::CoreDisconnected { error } => {
                        managed.status = WorkspaceStatus::Disconnected;
                        self.notices.push(WorkspaceNotice {
                            workspace: workspace.clone(),
                            message: format!("core stopped: {error}"),
                        });
                    }
                    FrontEvent::CoreRestarted { resumed } if *workspace != active_workspace => {
                        managed.status = WorkspaceStatus::Running;
                        self.notices.push(WorkspaceNotice {
                            workspace: workspace.clone(),
                            message: if resumed {
                                "core restarted and resumed its session".to_owned()
                            } else {
                                "core restarted".to_owned()
                            },
                        });
                    }
                    FrontEvent::TurnEnded { reason } if *workspace != active_workspace => {
                        self.notices.push(WorkspaceNotice {
                            workspace: workspace.clone(),
                            message: format!("background turn ended: {reason:?}"),
                        });
                    }
                    FrontEvent::InteractionRequested { kind } if *workspace != active_workspace => {
                        self.notices.push(WorkspaceNotice {
                            workspace: workspace.clone(),
                            message: format!("background core needs attention: {kind}"),
                        });
                    }
                    _ => {}
                }
            }
        }

        if let Some(error) = error {
            return Err(error);
        }
        Ok(poll)
    }

    /// Take queued background notifications.
    pub fn take_notices(&mut self) -> Vec<WorkspaceNotice> {
        std::mem::take(&mut self.notices)
    }
}

impl Deref for FrontRuntimeManager {
    type Target = FrontRuntime;

    fn deref(&self) -> &Self::Target {
        &self
            .runtimes
            .get(&self.active_workspace)
            .expect("active workspace runtime exists")
            .runtime
    }
}

impl DerefMut for FrontRuntimeManager {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self
            .runtimes
            .get_mut(&self.active_workspace)
            .expect("active workspace runtime exists")
            .runtime
    }
}

fn normalize_workspace(workspace: &Path) -> PathBuf {
    workspace
        .canonicalize()
        .unwrap_or_else(|_| workspace.to_path_buf())
}
