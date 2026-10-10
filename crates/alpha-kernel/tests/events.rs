use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use alpha_kernel::{BailEvent, Ctx, EmitEvent, Event, ParallelEvent, SerialEvent, WaterfallEvent};

#[derive(Debug)]
struct Note(String);

impl Event for Note {}
impl EmitEvent for Note {}

struct Rewrite {
    text: String,
}

impl Event for Rewrite {}
impl WaterfallEvent for Rewrite {}

struct Fan {
    hits: Arc<AtomicUsize>,
}

impl Event for Fan {}
impl ParallelEvent for Fan {}

struct Chain(Vec<&'static str>);

impl Event for Chain {}
impl SerialEvent for Chain {}

struct Decision(&'static str);

impl Event for Decision {}

impl BailEvent for Decision {
    type Outcome = &'static str;
}

#[test]
fn emit_runs_from_root_to_leaf_and_ignores_child_handlers_upward() {
    let parent = Ctx::root();
    let child = parent.child().expect("child");
    let seen = Arc::new(Mutex::new(Vec::new()));

    let parent_seen = seen.clone();
    parent
        .on_emit::<Note, _>(move |event| {
            parent_seen
                .lock()
                .expect("lock")
                .push(format!("parent:{}", event.0));
        })
        .expect("handler");
    let child_seen = seen.clone();
    child
        .on_emit::<Note, _>(move |event| {
            child_seen
                .lock()
                .expect("lock")
                .push(format!("child:{}", event.0));
        })
        .expect("handler");

    child.emit(Note("hello".to_owned()));
    assert_eq!(
        *seen.lock().expect("lock"),
        vec!["parent:hello", "child:hello"]
    );

    seen.lock().expect("lock").clear();
    parent.emit(Note("hello".to_owned()));
    assert_eq!(*seen.lock().expect("lock"), vec!["parent:hello"]);
}

#[tokio::test]
async fn waterfall_rewrites_and_short_circuits() {
    let ctx = Ctx::root();
    ctx.on_waterfall::<Rewrite, _>(|event, next| {
        Box::pin(async move {
            event.text.push_str("-a");
            next.run(event).await;
        })
    })
    .expect("handler");
    ctx.on_waterfall::<Rewrite, _>(|event, next| {
        Box::pin(async move {
            if event.text.starts_with("stop") {
                return;
            }
            event.text.push_str("-b");
            next.run(event).await;
        })
    })
    .expect("handler");
    ctx.on_waterfall::<Rewrite, _>(|event, _next| {
        Box::pin(async move {
            event.text.push_str("-c");
        })
    })
    .expect("handler");

    let mut event = Rewrite {
        text: "go".to_owned(),
    };
    ctx.waterfall(&mut event).await;
    assert_eq!(event.text, "go-a-b-c");

    let mut stopped = Rewrite {
        text: "stop".to_owned(),
    };
    ctx.waterfall(&mut stopped).await;
    assert_eq!(stopped.text, "stop-a");
}

#[tokio::test]
async fn parallel_waits_for_every_observer() {
    let ctx = Ctx::root();
    let hits = Arc::new(AtomicUsize::new(0));
    for _ in 0..2 {
        ctx.on_parallel::<Fan, _>(move |event| {
            let counter = event.hits.clone();
            Box::pin(async move {
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                counter.fetch_add(1, Ordering::SeqCst);
            })
        })
        .expect("handler");
    }

    ctx.parallel(Fan { hits: hits.clone() }).await;
    assert_eq!(hits.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn serial_runs_handlers_in_order() {
    let ctx = Ctx::root();
    ctx.on_serial::<Chain, _>(|event| {
        Box::pin(async move {
            event.0.push("first");
        })
    })
    .expect("handler");
    ctx.on_serial::<Chain, _>(|event| {
        Box::pin(async move {
            event.0.push("second");
        })
    })
    .expect("handler");

    let mut chain = Chain(Vec::new());
    ctx.serial(&mut chain).await;
    assert_eq!(chain.0, vec!["first", "second"]);
}

#[test]
fn bail_stops_at_the_first_outcome() {
    let ctx = Ctx::root();
    let late = Arc::new(AtomicBool::new(false));

    ctx.on_bail::<Decision, _>(|_| None).expect("handler");
    ctx.on_bail::<Decision, _>(|event| (event.0 == "yes").then_some("matched"))
        .expect("handler");
    let late_flag = late.clone();
    ctx.on_bail::<Decision, _>(move |_| {
        late_flag.store(true, Ordering::SeqCst);
        Some("late")
    })
    .expect("handler");

    assert_eq!(ctx.bail(&Decision("yes")), Some("matched"));
    assert!(!late.load(Ordering::SeqCst));
    assert_eq!(ctx.bail(&Decision("no")), Some("late"));
    assert!(late.load(Ordering::SeqCst));
}

#[test]
fn disposing_a_handler_removes_it() {
    let ctx = Ctx::root();
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = hits.clone();
    let disposer = ctx
        .on_emit::<Note, _>(move |_| {
            counter.fetch_add(1, Ordering::SeqCst);
        })
        .expect("handler");

    ctx.emit(Note("first".to_owned()));
    disposer.dispose();
    ctx.emit(Note("second".to_owned()));

    assert_eq!(hits.load(Ordering::SeqCst), 1);
}
