//! Scoped, reversible effects.
//!
//! Every registration in the kernel (services, event handlers, tasks) is an
//! effect owned by a [`Scope`]. Disposing a scope unwinds its effects in
//! reverse registration order and cascades to child scopes.

use std::fmt;
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};

use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::error::KernelError;

/// A handle to a single registered effect.
///
/// Disposing is idempotent. The owning scope keeps the effect alive; dropping
/// this handle does not dispose it.
pub struct Disposer {
    handle: Arc<EffectHandle>,
}

impl Disposer {
    /// Dispose the effect. Returns `true` when this call performed the
    /// disposal and `false` when it was already disposed.
    pub fn dispose(&self) -> bool {
        self.handle.dispose()
    }

    /// Whether the effect has been disposed.
    pub fn is_disposed(&self) -> bool {
        self.handle.is_disposed()
    }
}

impl fmt::Debug for Disposer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Disposer")
            .field("disposed", &self.is_disposed())
            .finish()
    }
}

struct EffectHandle {
    disposer: Mutex<Option<Box<dyn FnOnce() + Send + 'static>>>,
}

impl EffectHandle {
    fn new<F>(disposer: F) -> Arc<Self>
    where
        F: FnOnce() + Send + 'static,
    {
        Arc::new(Self {
            disposer: Mutex::new(Some(Box::new(disposer))),
        })
    }

    fn dispose(&self) -> bool {
        let disposer = self.disposer.lock().expect("effect lock poisoned").take();
        match disposer {
            Some(disposer) => {
                disposer();
                true
            }
            None => false,
        }
    }

    fn is_disposed(&self) -> bool {
        self.disposer
            .lock()
            .expect("effect lock poisoned")
            .is_none()
    }
}

/// A handle to a task spawned through [`Scope::spawn`].
pub struct TaskHandle {
    join: JoinHandle<()>,
    token: CancellationToken,
    _scope_effect: Disposer,
}

impl TaskHandle {
    /// Request cooperative cancellation.
    pub fn cancel(&self) {
        self.token.cancel();
    }

    /// Abort the task immediately.
    pub fn abort(&self) {
        self.join.abort();
    }

    /// Whether the task has finished.
    pub fn is_finished(&self) -> bool {
        self.join.is_finished()
    }

    /// Wait for the task to finish.
    pub async fn join(self) -> Result<(), tokio::task::JoinError> {
        self.join.await
    }
}

impl fmt::Debug for TaskHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TaskHandle")
            .field("finished", &self.is_finished())
            .finish()
    }
}

/// A scope owns effects and a cancellation token.
///
/// Cloning a scope shares the same underlying state. Disposing a scope unwinds
/// effects in reverse order and cancels every task spawned through it.
#[derive(Clone)]
pub struct Scope {
    inner: Arc<ScopeInner>,
}

struct ScopeInner {
    effects: Mutex<Vec<Arc<EffectHandle>>>,
    children: Mutex<Vec<Weak<ScopeInner>>>,
    disposed: AtomicBool,
    cancellation: CancellationToken,
}

impl Scope {
    /// Create a scope with no parent.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(ScopeInner {
                effects: Mutex::new(Vec::new()),
                children: Mutex::new(Vec::new()),
                disposed: AtomicBool::new(false),
                cancellation: CancellationToken::new(),
            }),
        }
    }

    /// Register a reversible effect.
    pub fn effect<F>(&self, effect: F) -> Result<Disposer, KernelError>
    where
        F: FnOnce() + Send + 'static,
    {
        if self.is_disposed() {
            return Err(KernelError::ScopeClosed);
        }
        let handle = EffectHandle::new(effect);
        let mut effects = self.inner.effects.lock().expect("scope lock poisoned");
        if self.is_disposed() {
            return Err(KernelError::ScopeClosed);
        }
        effects.push(handle.clone());
        drop(effects);
        Ok(Disposer { handle })
    }

    /// Create a child scope. Disposing this scope disposes the child.
    pub fn child(&self) -> Result<Scope, KernelError> {
        if self.is_disposed() {
            return Err(KernelError::ScopeClosed);
        }
        let child = Scope::new();
        {
            let mut children = self.inner.children.lock().expect("scope lock poisoned");
            children.retain(|child| child.strong_count() > 0);
            children.push(Arc::downgrade(&child.inner));
        }
        Ok(child)
    }

    /// Dispose the scope and unwind its effects in reverse order.
    pub fn dispose(&self) {
        if self.inner.disposed.swap(true, Ordering::SeqCst) {
            return;
        }
        self.inner.cancellation.cancel();

        let children: Vec<Scope> = {
            let mut children = self.inner.children.lock().expect("scope lock poisoned");
            let scopes = children
                .iter()
                .filter_map(Weak::upgrade)
                .map(|inner| Scope { inner })
                .collect();
            children.clear();
            scopes
        };
        for child in children {
            child.dispose();
        }

        let effects = std::mem::take(&mut *self.inner.effects.lock().expect("scope lock poisoned"));
        for effect in effects.into_iter().rev() {
            effect.dispose();
        }
    }

    /// Whether the scope has been disposed.
    pub fn is_disposed(&self) -> bool {
        self.inner.disposed.load(Ordering::SeqCst)
    }

    /// The cancellation token for this scope.
    pub fn cancellation_token(&self) -> CancellationToken {
        self.inner.cancellation.clone()
    }

    /// Spawn a task bound to this scope.
    ///
    /// The task races the future against the scope's cancellation token, so
    /// disposing the scope cancels it at the next await point. The returned
    /// handle can also cancel or abort the task directly.
    pub fn spawn<F>(&self, future: F) -> Result<TaskHandle, KernelError>
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let token = self.inner.cancellation.child_token();
        let cancel = token.clone();
        let scope_effect = self.effect(move || cancel.cancel())?;
        let task_token = token.clone();
        let join = tokio::spawn(async move {
            tokio::select! {
                _ = task_token.cancelled() => {}
                _ = future => {}
            }
        });
        Ok(TaskHandle {
            join,
            token,
            _scope_effect: scope_effect,
        })
    }
}

impl Default for Scope {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for Scope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Scope")
            .field("disposed", &self.is_disposed())
            .finish()
    }
}
