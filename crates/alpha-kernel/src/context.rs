//! Contexts and the singleton service registry.
//!
//! A [`Ctx`] is a node in a context tree. Service lookups walk from the
//! current context to its ancestors; providing a service in a child context
//! shadows the same type in its parent. Providing the same type twice in one
//! context is a conflict.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock, Weak};

use crate::effect::{Disposer, Scope};
use crate::error::KernelError;
use crate::event::EventRegistry;

/// A change to a singleton service in a context subtree.
pub enum ServiceChange<T: ?Sized + 'static> {
    /// A service became available.
    Registered(Arc<T>),
    /// A service is no longer available.
    Unregistered,
}

impl<T: ?Sized + 'static> Clone for ServiceChange<T> {
    fn clone(&self) -> Self {
        match self {
            Self::Registered(service) => Self::Registered(service.clone()),
            Self::Unregistered => Self::Unregistered,
        }
    }
}

/// A context in the runtime tree.
#[derive(Clone)]
pub struct Ctx {
    inner: Arc<CtxInner>,
}

struct CtxInner {
    parent: Option<Ctx>,
    scope: Scope,
    services: RwLock<HashMap<TypeId, ErasedService>>,
    watchers: RwLock<HashMap<TypeId, Vec<WatcherEntry>>>,
    children: RwLock<Vec<Weak<CtxInner>>>,
    watcher_seq: AtomicU64,
    events: EventRegistry,
}

struct ErasedService {
    value: Box<dyn Any + Send + Sync>,
}

struct Erased<T: ?Sized + 'static> {
    value: Arc<T>,
}

struct WatcherEntry {
    id: u64,
    callback: Box<dyn Any + Send + Sync>,
}

impl Ctx {
    /// Create a root context with a fresh scope.
    pub fn root() -> Self {
        Self {
            inner: Arc::new(CtxInner {
                parent: None,
                scope: Scope::new(),
                services: RwLock::new(HashMap::new()),
                watchers: RwLock::new(HashMap::new()),
                children: RwLock::new(Vec::new()),
                watcher_seq: AtomicU64::new(1),
                events: EventRegistry::default(),
            }),
        }
    }

    /// Create a child context. The child shares its ancestor services and is
    /// disposed together with its parent.
    pub fn child(&self) -> Result<Self, KernelError> {
        let scope = self.inner.scope.child()?;
        let child = Self {
            inner: Arc::new(CtxInner {
                parent: Some(self.clone()),
                scope,
                services: RwLock::new(HashMap::new()),
                watchers: RwLock::new(HashMap::new()),
                children: RwLock::new(Vec::new()),
                watcher_seq: AtomicU64::new(1),
                events: EventRegistry::default(),
            }),
        };
        self.inner
            .children
            .write()
            .expect("context lock poisoned")
            .push(Arc::downgrade(&child.inner));
        Ok(child)
    }

    /// The parent context, if any.
    pub fn parent(&self) -> Option<&Ctx> {
        self.inner.parent.as_ref()
    }

    /// The scope owned by this context.
    pub fn scope(&self) -> &Scope {
        &self.inner.scope
    }

    /// Dispose this context and every effect registered through it.
    pub fn dispose(&self) {
        self.inner.scope.dispose();
    }

    /// Whether this context has been disposed.
    pub fn is_disposed(&self) -> bool {
        self.inner.scope.is_disposed()
    }

    pub(crate) fn events(&self) -> &EventRegistry {
        &self.inner.events
    }

    /// Provide a singleton service in this context.
    ///
    /// Providing the same service type twice in one context is a conflict.
    /// Child contexts may shadow a service from an ancestor.
    pub fn provide<T>(&self, service: Arc<T>) -> Result<Disposer, KernelError>
    where
        T: ?Sized + Send + Sync + 'static,
    {
        if self.is_disposed() {
            return Err(KernelError::ScopeClosed);
        }
        let type_id = TypeId::of::<T>();
        {
            let mut services = self.inner.services.write().expect("service lock poisoned");
            if services.contains_key(&type_id) {
                return Err(KernelError::ServiceConflict {
                    type_name: std::any::type_name::<T>(),
                });
            }
            services.insert(
                type_id,
                ErasedService {
                    value: Box::new(Erased {
                        value: service.clone(),
                    }),
                },
            );
        }
        self.notify_service::<T>(ServiceChange::Registered(service));

        let ctx = self.clone();
        match self.inner.scope.effect(move || {
            let removed = ctx
                .inner
                .services
                .write()
                .expect("service lock poisoned")
                .remove(&type_id)
                .is_some();
            if removed {
                ctx.notify_service::<T>(ServiceChange::Unregistered);
            }
        }) {
            Ok(disposer) => Ok(disposer),
            Err(error) => {
                self.inner
                    .services
                    .write()
                    .expect("service lock poisoned")
                    .remove(&type_id);
                Err(error)
            }
        }
    }

    /// Look up a service in this context or its ancestors.
    pub fn get<T>(&self) -> Option<Arc<T>>
    where
        T: ?Sized + Send + Sync + 'static,
    {
        let type_id = TypeId::of::<T>();
        let mut current = Some(self);
        while let Some(ctx) = current {
            let services = ctx.inner.services.read().expect("service lock poisoned");
            if let Some(entry) = services.get(&type_id)
                && let Some(erased) = entry.value.downcast_ref::<Erased<T>>()
            {
                return Some(erased.value.clone());
            }
            drop(services);
            current = ctx.parent();
        }
        None
    }

    /// Require a service, returning an error when it is not available.
    pub fn require<T>(&self) -> Result<Arc<T>, KernelError>
    where
        T: ?Sized + Send + Sync + 'static,
    {
        self.get::<T>().ok_or(KernelError::MissingService {
            type_name: std::any::type_name::<T>(),
        })
    }

    /// Watch a service type in this context and its descendants.
    ///
    /// The handler runs immediately when the service is currently available,
    /// then again on every registration or removal in the subtree.
    pub fn watch<T, F>(&self, handler: F) -> Result<Disposer, KernelError>
    where
        T: ?Sized + Send + Sync + 'static,
        F: Fn(ServiceChange<T>) + Send + Sync + 'static,
    {
        let type_id = TypeId::of::<T>();
        let callback: Arc<dyn Fn(ServiceChange<T>) + Send + Sync> = Arc::new(handler);
        if let Some(service) = self.get::<T>() {
            callback(ServiceChange::Registered(service));
        }
        let id = self.inner.watcher_seq.fetch_add(1, Ordering::Relaxed);
        {
            let mut watchers = self.inner.watchers.write().expect("watcher lock poisoned");
            watchers.entry(type_id).or_default().push(WatcherEntry {
                id,
                callback: Box::new(callback),
            });
        }
        let ctx = self.clone();
        self.inner.scope.effect(move || {
            let mut watchers = ctx.inner.watchers.write().expect("watcher lock poisoned");
            if let Some(entries) = watchers.get_mut(&type_id) {
                entries.retain(|entry| entry.id != id);
            }
        })
    }

    fn notify_service<T>(&self, change: ServiceChange<T>)
    where
        T: ?Sized + Send + Sync + 'static,
    {
        let type_id = TypeId::of::<T>();
        let mut callbacks: Vec<Arc<dyn Fn(ServiceChange<T>) + Send + Sync>> = Vec::new();
        self.collect_watchers::<T>(&type_id, &mut callbacks);
        for callback in callbacks {
            callback(change.clone());
        }
    }

    fn collect_watchers<T>(
        &self,
        type_id: &TypeId,
        out: &mut Vec<Arc<dyn Fn(ServiceChange<T>) + Send + Sync>>,
    ) where
        T: ?Sized + Send + Sync + 'static,
    {
        {
            let watchers = self.inner.watchers.read().expect("watcher lock poisoned");
            if let Some(entries) = watchers.get(type_id) {
                for entry in entries {
                    if let Some(callback) = entry
                        .callback
                        .downcast_ref::<Arc<dyn Fn(ServiceChange<T>) + Send + Sync>>()
                    {
                        out.push(callback.clone());
                    }
                }
            }
        }
        let children: Vec<Ctx> = {
            let mut children = self.inner.children.write().expect("context lock poisoned");
            children.retain(|child| child.strong_count() > 0);
            children
                .iter()
                .filter_map(Weak::upgrade)
                .map(|inner| Ctx { inner })
                .collect()
        };
        for child in children {
            child.collect_watchers::<T>(type_id, out);
        }
    }
}

impl Default for Ctx {
    fn default() -> Self {
        Self::root()
    }
}
