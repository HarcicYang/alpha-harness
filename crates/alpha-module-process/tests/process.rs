use std::time::Duration;

use alpha_kernel::{Ctx, Module};
use alpha_module_process::{
    LocalProcessRunner, ProcessConfig, ProcessError, ProcessLocalModule, ProcessRequest,
    ProcessRunner,
};

#[cfg(unix)]
#[tokio::test]
async fn runs_commands_in_the_fixed_cwd_and_captures_output() {
    let dir = tempfile::tempdir().expect("tempdir");
    let runner = LocalProcessRunner::new(dir.path());
    let outcome = runner
        .run(ProcessRequest::new(
            "sh",
            ["-c".to_owned(), "printf out; printf err >&2".to_owned()],
        ))
        .await
        .expect("run");

    assert!(outcome.success);
    assert_eq!(outcome.stdout, b"out");
    assert_eq!(outcome.stderr, b"err");
}

#[cfg(unix)]
#[tokio::test]
async fn failing_exit_codes_are_reported() {
    let dir = tempfile::tempdir().expect("tempdir");
    let runner = LocalProcessRunner::new(dir.path());
    let outcome = runner
        .run(ProcessRequest::new(
            "sh",
            ["-c".to_owned(), "exit 3".to_owned()],
        ))
        .await
        .expect("run");

    assert!(!outcome.success);
    assert_eq!(outcome.code, Some(3));
}

#[cfg(unix)]
#[tokio::test]
async fn timeouts_kill_the_process() {
    let dir = tempfile::tempdir().expect("tempdir");
    let runner = LocalProcessRunner::new(dir.path());
    let error = runner
        .run(
            ProcessRequest::new("sh", ["-c".to_owned(), "sleep 5".to_owned()])
                .with_timeout(Duration::from_millis(50)),
        )
        .await
        .expect_err("timeout");

    assert!(matches!(error, ProcessError::Timeout(_)));
}

#[tokio::test]
async fn module_provides_the_seam() {
    let dir = tempfile::tempdir().expect("tempdir");
    let ctx = Ctx::root();
    ProcessLocalModule
        .activate(
            &ctx,
            &ProcessConfig {
                cwd: dir.path().to_path_buf(),
            },
        )
        .expect("activate");

    let runner = ctx.require::<dyn ProcessRunner>().expect("seam");
    let outcome = runner
        .run(ProcessRequest::new("rustc", ["--version".to_owned()]))
        .await
        .expect("run rustc");
    assert!(outcome.success);
    assert!(String::from_utf8_lossy(&outcome.stdout).contains("rustc"));

    ctx.dispose();
    assert!(ctx.get::<dyn ProcessRunner>().is_none());
}
