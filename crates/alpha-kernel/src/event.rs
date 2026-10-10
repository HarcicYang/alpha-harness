//! Typed events with five dispatch modes.
//!
//! Every event type implements exactly one dispatch trait. The dispatch
//! methods are bounded by those traits, so calling the wrong method for an
//! event is a compile-time error.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use futures::future::BoxFuture;

use crate::context::Ctx;
use crate::effect::Disposer;
use crate::error::KernelError;

/// Base marker for every event type.
pub trait Event: Send + Sync + 'static {}

/// Observe an event. Handlers are synchronous and return no value.
pub trait EmitEvent: Event {}

/// Around-middleware. A handler receives the event and `next`, and decides
/// whether to delegate by calling `next.run(event)`.
pub trait WaterfallEvent: Event {}

/// Fan out to asynchronous observers that all run to completion.
pub trait ParallelEvent: Event {}

/// Run asynchronous handlers in order, mutating the event.
pub trait SerialEvent: Event {}

/// Run synchronous handlers in order until one returns `Some`.
pub trait BailEvent: Event {
    /// The value produced by the first handler that bails.
    type Outcome: Send + Sync + 'static;
}

type BucketMap = RwLock<HashMap<TypeId, Box<dyn Any + Send + Sync>>>;

type EmitHandler<E> = Arc<dyn Fn(&E) + Send + Sync>;
type ParallelHandler<E> = Arc<dyn Fn(Arc<E>) -> BoxFuture<'static, ()> + Send + Sync>;
type BailHandler<E> = Arc<dyn Fn(&E) -> Option<<E as BailEvent>::Outcome> + Send + Sync>;

/// Handler contract for waterfall events.
pub trait WaterfallHandler<E: WaterfallEvent>: Send + Sync + 'static {
    /// Handle the event and optionally delegate to the next handler.
    fn handle<'a>(&'a self, event: &'a mut E, next: Next<'a, E>) -> BoxFuture<'a, ()>;
}

impl<E, F> WaterfallHandler<E> for F
where
    E: WaterfallEvent,
    F: for<'a> Fn(&'a mut E, Next<'a, E>) -> BoxFuture<'a, ()> + Send + Sync + 'static,
{
    fn handle<'a>(&'a self, event: &'a mut E, next: Next<'a, E>) -> BoxFuture<'a, ()> {
        self(event, next)
    }
}

/// Handler contract for serial events.
pub trait SerialHandler<E: SerialEvent>: Send + Sync + 'static {
    /// Handle the event, awaited in registration order.
    fn handle<'a>(&'a self, event: &'a mut E) -> BoxFuture<'a, ()>;
}

impl<E, F> SerialHandler<E> for F
where
    E: SerialEvent,
    F: for<'a> Fn(&'a mut E) -> BoxFuture<'a, ()> + Send + Sync + 'static,
{
    fn handle<'a>(&'a self, event: &'a mut E) -> BoxFuture<'a, ()> {
        self(event)
    }
}

/// The continuation passed to waterfall handlers.
pub struct Next<'a, E: WaterfallEvent> {
    handlers: &'a [Arc<dyn WaterfallHandler<E>>],
    index: usize,
}

impl<'a, E: WaterfallEvent> Next<'a, E> {
    /// Run the remaining handler chain. Not calling this short-circuits the
    /// waterfall.
    pub fn run(self, event: &'a mut E) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            if let Some(handler) = self.handlers.get(self.index) {
                let next = Next {
                    handlers: self.handlers,
                    index: self.index + 1,
                };
                handler.handle(event, next).await;
            }
        })
    }

    /// Whether no further handlers remain.
    pub fn is_last(&self) -> bool {
        self.index >= self.handlers.len()
    }
}

#[derive(Default)]
pub(crate) struct EventRegistry {
    pub(crate) emit: BucketMap,
    pub(crate) waterfall: BucketMap,
    pub(crate) parallel: BucketMap,
    pub(crate) serial: BucketMap,
    pub(crate) bail: BucketMap,
}

fn emit_bucket(registry: &EventRegistry) -> &BucketMap {
    &registry.emit
}

fn waterfall_bucket(registry: &EventRegistry) -> &BucketMap {
    &registry.waterfall
}

fn parallel_bucket(registry: &EventRegistry) -> &BucketMap {
    &registry.parallel
}

fn serial_bucket(registry: &EventRegistry) -> &BucketMap {
    &registry.serial
}

fn bail_bucket(registry: &EventRegistry) -> &BucketMap {
    &registry.bail
}

fn with_bucket<H, R>(
    map: &BucketMap,
    type_id: TypeId,
    action: impl FnOnce(&mut Vec<Arc<H>>) -> R,
) -> R
where
    H: ?Sized + Send + Sync + 'static,
{
    let mut buckets = map.write().expect("event lock poisoned");
    let bucket = buckets
        .entry(type_id)
        .or_insert_with(|| Box::new(Vec::<Arc<H>>::new()));
    let handlers = bucket
        .downcast_mut::<Vec<Arc<H>>>()
        .expect("event bucket type mismatch");
    action(handlers)
}

fn collect<H>(ctx: &Ctx, select: fn(&EventRegistry) -> &BucketMap, type_id: TypeId) -> Vec<Arc<H>>
where
    H: ?Sized + Send + Sync + 'static,
{
    let mut chain = Vec::new();
    let mut current = Some(ctx);
    while let Some(node) = current {
        chain.push(node);
        current = node.parent();
    }
    chain.reverse();

    let mut handlers = Vec::new();
    for node in chain {
        let buckets = select(node.events()).read().expect("event lock poisoned");
        if let Some(bucket) = buckets.get(&type_id) {
            if let Some(list) = bucket.downcast_ref::<Vec<Arc<H>>>() {
                handlers.extend(list.iter().cloned());
            }
        }
    }
    handlers
}

impl Ctx {
    fn register_handler<H>(
        &self,
        select: fn(&EventRegistry) -> &BucketMap,
        type_id: TypeId,
        handler: Arc<H>,
    ) -> Result<Disposer, KernelError>
    where
        H: ?Sized + Send + Sync + 'static,
    {
        with_bucket::<H, _>(select(self.events()), type_id, |list| {
            list.push(handler.clone());
        });

        let ctx = self.clone();
        let target = handler.clone();
        match self.scope().effect(move || {
            with_bucket::<H, _>(select(ctx.events()), type_id, |list| {
                list.retain(|item| !Arc::ptr_eq(item, &target));
            });
        }) {
            Ok(disposer) => Ok(disposer),
            Err(error) => {
                with_bucket::<H, _>(select(self.events()), type_id, |list| {
                    list.retain(|item| !Arc::ptr_eq(item, &handler));
                });
                Err(error)
            }
        }
    }

    /// Register a synchronous observer for an [`EmitEvent`].
    pub fn on_emit<E, F>(&self, handler: F) -> Result<Disposer, KernelError>
    where
        E: EmitEvent,
        F: Fn(&E) + Send + Sync + 'static,
    {
        let handler: EmitHandler<E> = Arc::new(handler);
        self.register_handler(emit_bucket, TypeId::of::<E>(), handler)
    }

    /// Register a waterfall middleware for a [`WaterfallEvent`].
    pub fn on_waterfall<E, F>(&self, handler: F) -> Result<Disposer, KernelError>
    where
        E: WaterfallEvent,
        F: for<'a> Fn(&'a mut E, Next<'a, E>) -> BoxFuture<'a, ()> + Send + Sync + 'static,
    {
        let handler: Arc<dyn WaterfallHandler<E>> = Arc::new(handler);
        self.register_handler(waterfall_bucket, TypeId::of::<E>(), handler)
    }

    /// Register an asynchronous observer for a [`ParallelEvent`].
    pub fn on_parallel<E, F>(&self, handler: F) -> Result<Disposer, KernelError>
    where
        E: ParallelEvent,
        F: Fn(Arc<E>) -> BoxFuture<'static, ()> + Send + Sync + 'static,
    {
        let handler: ParallelHandler<E> = Arc::new(handler);
        self.register_handler(parallel_bucket, TypeId::of::<E>(), handler)
    }

    /// Register an asynchronous handler for a [`SerialEvent`].
    pub fn on_serial<E, F>(&self, handler: F) -> Result<Disposer, KernelError>
    where
        E: SerialEvent,
        F: for<'a> Fn(&'a mut E) -> BoxFuture<'a, ()> + Send + Sync + 'static,
    {
        let handler: Arc<dyn SerialHandler<E>> = Arc::new(handler);
        self.register_handler(serial_bucket, TypeId::of::<E>(), handler)
    }

    /// Register a synchronous handler for a [`BailEvent`].
    pub fn on_bail<E, F>(&self, handler: F) -> Result<Disposer, KernelError>
    where
        E: BailEvent,
        F: Fn(&E) -> Option<E::Outcome> + Send + Sync + 'static,
    {
        let handler: BailHandler<E> = Arc::new(handler);
        self.register_handler(bail_bucket, TypeId::of::<E>(), handler)
    }

    /// Dispatch an [`EmitEvent`] to synchronous observers, root to leaf.
    pub fn emit<E: EmitEvent>(&self, event: E) {
        let handlers: Vec<EmitHandler<E>> = collect(self, emit_bucket, TypeId::of::<E>());
        for handler in handlers {
            handler(&event);
        }
    }

    /// Dispatch a [`WaterfallEvent`] through its middleware chain.
    pub async fn waterfall<E: WaterfallEvent>(&self, event: &mut E) {
        let handlers: Vec<Arc<dyn WaterfallHandler<E>>> =
            collect(self, waterfall_bucket, TypeId::of::<E>());
        let next = Next {
            handlers: &handlers,
            index: 0,
        };
        next.run(event).await;
    }

    /// Dispatch a [`ParallelEvent`] to asynchronous observers and wait for all
    /// of them.
    pub async fn parallel<E: ParallelEvent>(&self, event: E) {
        let handlers: Vec<ParallelHandler<E>> = collect(self, parallel_bucket, TypeId::of::<E>());
        if handlers.is_empty() {
            return;
        }
        let event = Arc::new(event);
        let futures = handlers.iter().map(|handler| handler(event.clone()));
        futures::future::join_all(futures).await;
    }

    /// Dispatch a [`SerialEvent`] to asynchronous handlers in order.
    pub async fn serial<E: SerialEvent>(&self, event: &mut E) {
        let handlers: Vec<Arc<dyn SerialHandler<E>>> =
            collect(self, serial_bucket, TypeId::of::<E>());
        for handler in handlers {
            handler.handle(event).await;
        }
    }

    /// Dispatch a [`BailEvent`] until a handler returns an outcome.
    pub fn bail<E: BailEvent>(&self, event: &E) -> Option<E::Outcome> {
        let handlers: Vec<BailHandler<E>> = collect(self, bail_bucket, TypeId::of::<E>());
        for handler in handlers {
            if let Some(outcome) = handler(event) {
                return Some(outcome);
            }
        }
        None
    }
}
