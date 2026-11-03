use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use alpha_kernel::{Ctx, Module};
use alpha_module_process::{
    LocalProcessRunner, ProcessAccess, ProcessConfig, ProcessError, ProcessLocalModule,
    ProcessOutputCallback, ProcessRequest, ProcessRunner, ProcessRunnerSet, ProcessStream,
    SandboxConfig, available_bwrap, bwrap_args,
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
async fn streams_output_before_the_process_finishes() {
    let dir = tempfile::tempdir().expect("tempdir");
    let runner = LocalProcessRunner::new(dir.path());
    let chunks = Arc::new(Mutex::new(Vec::new()));
    let collected = chunks.clone();
    let output: ProcessOutputCallback = Arc::new(move |stream, bytes| {
        collected
            .lock()
            .expect("chunks lock poisoned")
            .push((stream, String::from_utf8_lossy(&bytes).into_owned()));
    });
    let outcome = runner
        .run_streaming(
            ProcessRequest::new(
                "sh",
                [
                    "-c".to_owned(),
                    "printf first; sleep 0.05; printf second >&2".to_owned(),
                ],
            ),
            output,
        )
        .await
        .expect("streaming run");

    assert!(outcome.success);
    let chunks = chunks.lock().expect("chunks lock poisoned");
    assert_eq!(
        chunks
            .iter()
            .filter(|(stream, _)| *stream == ProcessStream::Stdout)
            .map(|(_, text)| text.as_str())
            .collect::<String>(),
        "first"
    );
    assert_eq!(
        chunks
            .iter()
            .filter(|(stream, _)| *stream == ProcessStream::Stderr)
            .map(|(_, text)| text.as_str())
            .collect::<String>(),
        "second"
    );
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

#[cfg(unix)]
#[tokio::test]
async fn dropping_a_streaming_run_kills_descendants() {
    use nix::sys::signal::kill;
    use nix::unistd::Pid;

    let dir = tempfile::tempdir().expect("tempdir");
    let runner = LocalProcessRunner::new(dir.path());
    let seen = Arc::new(Mutex::new(None));
    let sink = seen.clone();
    let output: ProcessOutputCallback = Arc::new(move |_, bytes| {
        if let Ok(pid) = String::from_utf8_lossy(&bytes).trim().parse::<i32>() {
            *sink.lock().expect("pid lock poisoned") = Some(pid);
        }
    });
    let mut run = Box::pin(runner.run_streaming(
        ProcessRequest::new(
            "sh",
            ["-c".to_owned(), "sleep 30 & echo $!; wait".to_owned()],
        ),
        output,
    ));
    let pid = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            tokio::select! {
                result = &mut run => panic!("run finished before the pid was read: {result:?}"),
                _ = tokio::time::sleep(Duration::from_millis(10)) => {
                    if let Some(pid) = *seen.lock().expect("pid lock poisoned") {
                        break pid;
                    }
                }
            }
        }
    })
    .await
    .expect("pid");

    drop(run);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        kill(Pid::from_raw(pid), None).is_err(),
        "the descendant process survived the interrupted run"
    );
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
                ..ProcessConfig::default()
            },
        )
        .expect("activate");

    let runner = ctx.require::<dyn ProcessRunner>().expect("seam");
    let runners = ctx.require::<ProcessRunnerSet>().expect("runner set");
    assert_eq!(runners.sandbox_available(), available_bwrap().is_some());
    assert!(runners.get(ProcessAccess::FullAccess).is_ok());
    let outcome = runner
        .run(ProcessRequest::new("rustc", ["--version".to_owned()]))
        .await
        .expect("run rustc");
    assert!(outcome.success);
    assert!(String::from_utf8_lossy(&outcome.stdout).contains("rustc"));

    ctx.dispose();
    assert!(ctx.get::<dyn ProcessRunner>().is_none());
}

#[test]
fn bwrap_args_include_isolation_and_request() {
    let request = ProcessRequest::new("echo", ["hi".to_owned()]);
    let args = bwrap_args(Path::new("/workspace"), &SandboxConfig::default(), &request);
    assert!(args.contains(&"--unshare-net".to_owned()));
    assert!(args.contains(&"--unshare-pid".to_owned()));
    assert!(args.contains(&"/workspace".to_owned()));
    assert!(
        args.windows(3)
            .any(|window| { window == ["--ro-bind-try", "/workspace/.git", "/workspace/.git",] })
    );
    assert!(args.contains(&"echo".to_owned()));
    assert_eq!(args.last().map(String::as_str), Some("hi"));

    let networked = SandboxConfig {
        network: true,
        ..SandboxConfig::default()
    };
    let args = bwrap_args(Path::new("/workspace"), &networked, &request);
    assert!(!args.contains(&"--unshare-net".to_owned()));
}
