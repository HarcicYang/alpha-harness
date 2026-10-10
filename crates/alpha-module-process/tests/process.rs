use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use alpha_kernel::{Ctx, Module};
use alpha_module_process::{
    LocalProcessRunner, ProcessConfig, ProcessError, ProcessLocalModule, ProcessRequest,
    ProcessRunner, SandboxConfig, SandboxedProcessRunner, available_bwrap, bwrap_args,
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
                ..ProcessConfig::default()
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

#[test]
fn bwrap_args_include_isolation_and_request() {
    let request = ProcessRequest::new("echo", ["hi".to_owned()]);
    let args = bwrap_args(Path::new("/workspace"), &SandboxConfig::default(), &request);
    assert!(args.contains(&"--unshare-net".to_owned()));
    assert!(args.contains(&"--unshare-pid".to_owned()));
    assert!(args.contains(&"/workspace".to_owned()));
    assert!(args.contains(&"echo".to_owned()));
    assert_eq!(args.last().map(String::as_str), Some("hi"));

    let networked = SandboxConfig {
        network: true,
        ..SandboxConfig::default()
    };
    let args = bwrap_args(Path::new("/workspace"), &networked, &request);
    assert!(!args.contains(&"--unshare-net".to_owned()));
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn sandbox_confines_writes_and_network() {
    let Some(bwrap) = available_bwrap() else {
        eprintln!("bubblewrap is unavailable; skipping sandbox test");
        return;
    };
    let dir = tempfile::tempdir().expect("tempdir");
    let runner = SandboxedProcessRunner::new(
        Arc::new(LocalProcessRunner::new(dir.path())),
        dir.path().to_path_buf(),
        SandboxConfig::default(),
        bwrap,
    );

    let ok = runner
        .run(ProcessRequest::new(
            "sh",
            ["-c".to_owned(), "printf sandbox-ok".to_owned()],
        ))
        .await
        .expect("probe");
    assert!(ok.success);
    assert_eq!(ok.stdout, b"sandbox-ok");

    let inside = runner
        .run(ProcessRequest::new(
            "sh",
            ["-c".to_owned(), "printf data > inside.txt".to_owned()],
        ))
        .await
        .expect("inside write");
    assert!(inside.success);
    assert_eq!(
        std::fs::read_to_string(dir.path().join("inside.txt")).expect("file"),
        "data"
    );

    let denied = runner
        .run(ProcessRequest::new(
            "sh",
            ["-c".to_owned(), "touch /usr/sandbox-probe".to_owned()],
        ))
        .await
        .expect("outside write");
    assert!(!denied.success, "writes outside the workspace must fail");

    if std::process::Command::new("bash")
        .arg("--version")
        .output()
        .is_ok()
    {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("listener");
        let port = listener.local_addr().expect("addr").port();
        let command = format!("exec 3<>/dev/tcp/127.0.0.1/{port}");
        let network = runner
            .run(ProcessRequest::new("bash", ["-c".to_owned(), command]))
            .await
            .expect("network probe");
        assert!(!network.success, "network must be isolated by default");
    }
}
