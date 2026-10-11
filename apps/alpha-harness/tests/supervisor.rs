use alpha_front_supervisor::{CoreSupervisor, SupervisorConfig};

#[test]
fn hidden_core_handshakes_over_alpha_protocol() {
    let executable = env!("CARGO_BIN_EXE_alpha");
    let mut supervisor =
        CoreSupervisor::start_with_executable(executable, SupervisorConfig::default())
            .expect("start core");

    assert_eq!(supervisor.info().protocol, "alpha-protocol/1");
    assert_eq!(supervisor.info().pid, supervisor.pid());

    let pong = supervisor.request("alpha.ping", None).expect("core ping");
    assert_eq!(pong["pong"], serde_json::json!(true));

    supervisor.shutdown().expect("shutdown core");
}

#[test]
fn supervisor_reports_startup_attempts() {
    let missing = std::env::temp_dir().join("alpha-core-does-not-exist");
    let error = CoreSupervisor::start_with_executable(
        missing,
        SupervisorConfig {
            retries: 2,
            ..SupervisorConfig::default()
        },
    )
    .expect_err("missing core must fail");
    assert!(error.to_string().contains("after 3 attempts"));
}
