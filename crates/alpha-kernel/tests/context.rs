use std::sync::{Arc, Mutex};

use alpha_kernel::{Ctx, KernelError, ServiceChange};

trait Greeter: Send + Sync {
    fn greet(&self) -> String;
}

struct English;

impl Greeter for English {
    fn greet(&self) -> String {
        "hello".to_owned()
    }
}

struct Chinese;

impl Greeter for Chinese {
    fn greet(&self) -> String {
        "你好".to_owned()
    }
}

#[test]
fn effects_unwind_in_reverse_order() {
    let scope = alpha_kernel::Scope::new();
    let order = Arc::new(Mutex::new(Vec::new()));
    for value in [1, 2, 3] {
        let order = order.clone();
        scope
            .effect(move || order.lock().expect("lock").push(value))
            .expect("effect");
    }
    scope.dispose();
    assert_eq!(*order.lock().expect("lock"), vec![3, 2, 1]);
}

#[test]
fn disposing_a_scope_cascades_to_children() {
    let parent = Ctx::root();
    let child = parent.child().expect("child");
    let order = Arc::new(Mutex::new(Vec::new()));

    let parent_order = order.clone();
    parent
        .scope()
        .effect(move || parent_order.lock().expect("lock").push("parent"))
        .expect("effect");
    let child_order = order.clone();
    child
        .scope()
        .effect(move || child_order.lock().expect("lock").push("child"))
        .expect("effect");

    parent.dispose();
    assert!(child.is_disposed());
    assert_eq!(*order.lock().expect("lock"), vec!["child", "parent"]);
}

#[test]
fn services_are_looked_up_through_ancestors() {
    let parent = Ctx::root();
    let child = parent.child().expect("child");
    parent
        .provide::<dyn Greeter>(Arc::new(English))
        .expect("provide");

    let greeter = child.require::<dyn Greeter>().expect("service");
    assert_eq!(greeter.greet(), "hello");
}

#[test]
fn child_contexts_can_shadow_ancestor_services() {
    let parent = Ctx::root();
    let child = parent.child().expect("child");
    parent
        .provide::<dyn Greeter>(Arc::new(English))
        .expect("provide");
    child
        .provide::<dyn Greeter>(Arc::new(Chinese))
        .expect("provide");

    assert_eq!(
        parent.require::<dyn Greeter>().expect("parent").greet(),
        "hello"
    );
    assert_eq!(
        child.require::<dyn Greeter>().expect("child").greet(),
        "你好"
    );
}

#[test]
fn duplicate_singletons_conflict_in_one_context() {
    let ctx = Ctx::root();
    ctx.provide::<dyn Greeter>(Arc::new(English))
        .expect("provide");
    let error = ctx
        .provide::<dyn Greeter>(Arc::new(Chinese))
        .expect_err("conflict");
    assert!(matches!(error, KernelError::ServiceConflict { .. }));
}

#[test]
fn disposing_a_context_removes_its_services() {
    let ctx = Ctx::root();
    ctx.provide::<dyn Greeter>(Arc::new(English))
        .expect("provide");
    assert!(ctx.get::<dyn Greeter>().is_some());
    ctx.dispose();
    assert!(ctx.get::<dyn Greeter>().is_none());
}

#[test]
fn watchers_see_registration_and_removal() {
    let ctx = Ctx::root();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let seen_clone = seen.clone();
    let _watch = ctx
        .watch::<dyn Greeter, _>(move |change| {
            let label = match change {
                ServiceChange::Registered(_) => "registered",
                ServiceChange::Unregistered => "unregistered",
            };
            seen_clone.lock().expect("lock").push(label);
        })
        .expect("watch");

    let service = ctx
        .provide::<dyn Greeter>(Arc::new(English))
        .expect("provide");
    service.dispose();

    assert_eq!(
        *seen.lock().expect("lock"),
        vec!["registered", "unregistered"]
    );
}

#[test]
fn watchers_in_children_see_parent_registrations() {
    let parent = Ctx::root();
    let child = parent.child().expect("child");
    let seen = Arc::new(Mutex::new(Vec::new()));
    let seen_clone = seen.clone();
    let _watch = child
        .watch::<dyn Greeter, _>(move |change| {
            let label = match change {
                ServiceChange::Registered(_) => "registered",
                ServiceChange::Unregistered => "unregistered",
            };
            seen_clone.lock().expect("lock").push(label);
        })
        .expect("watch");

    let service = parent
        .provide::<dyn Greeter>(Arc::new(English))
        .expect("provide");
    service.dispose();

    assert_eq!(
        *seen.lock().expect("lock"),
        vec!["registered", "unregistered"]
    );
}

#[tokio::test]
async fn disposing_a_scope_cancels_spawned_tasks() {
    use std::sync::atomic::{AtomicBool, Ordering};

    let scope = alpha_kernel::Scope::new();
    let finished = Arc::new(AtomicBool::new(false));
    let finished_clone = finished.clone();
    let handle = scope
        .spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
            finished_clone.store(true, Ordering::SeqCst);
        })
        .expect("spawn");

    scope.dispose();
    handle.join().await.expect("join");
    assert!(!finished.load(Ordering::SeqCst));
}
