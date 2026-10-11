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

#[test]
fn core_exposes_provider_registry_methods() {
    let executable = env!("CARGO_BIN_EXE_alpha");
    let mut supervisor =
        CoreSupervisor::start_with_executable(executable, SupervisorConfig::default())
            .expect("start core");

    let kinds = supervisor
        .request("provider.list_kinds", None)
        .expect("list provider kinds");
    let kinds = kinds.as_array().expect("provider kinds");
    assert_eq!(kinds.len(), 5);

    let credential_status = supervisor
        .request("provider.credential_status", None)
        .expect("credential status");
    assert!(credential_status["backend"].is_string());
    assert!(credential_status["status"].is_string());

    let instance = serde_json::json!({
        "id": "test-openai",
        "kind": "openai-chat-completions",
        "config_version": 1,
        "config": { "base_url": "https://api.test/v1" }
    });
    let created = supervisor
        .request("provider.create_instance", Some(instance.clone()))
        .expect("create provider instance");
    assert_eq!(created["auth_ref"], "provider/test-openai/api-key");
    let listed = supervisor
        .request("provider.list_instances", None)
        .expect("list provider instances");
    assert_eq!(listed.as_array().expect("instances").len(), 1);

    supervisor
        .request(
            "provider.remove_instance",
            Some(serde_json::json!({ "id": "test-openai" })),
        )
        .expect("remove provider instance");
    supervisor.shutdown().expect("shutdown core");
}

#[test]
fn supervisor_restarts_a_crashed_core_without_replaying_turns() {
    let executable = env!("CARGO_BIN_EXE_alpha");
    let mut supervisor =
        CoreSupervisor::start_with_executable(executable, SupervisorConfig::default())
            .expect("start core");
    let old_pid = supervisor.pid();
    supervisor.restart().expect("restart core");
    assert_ne!(supervisor.pid(), old_pid);
    let pong = supervisor.request("alpha.ping", None).expect("ping");
    assert_eq!(pong["pong"], serde_json::json!(true));
    supervisor.shutdown().expect("shutdown core");
}

#[test]
fn independent_supervisors_run_in_parallel() {
    let executable = env!("CARGO_BIN_EXE_alpha");
    let mut first = CoreSupervisor::start_with_executable(executable, SupervisorConfig::default())
        .expect("start first core");
    let mut second = CoreSupervisor::start_with_executable(executable, SupervisorConfig::default())
        .expect("start second core");
    assert_ne!(first.pid(), second.pid());
    assert_eq!(
        first.request("alpha.ping", None).expect("first ping")["pong"],
        serde_json::json!(true)
    );
    assert_eq!(
        second.request("alpha.ping", None).expect("second ping")["pong"],
        serde_json::json!(true)
    );
    first.shutdown().expect("shutdown first core");
    assert_eq!(
        second
            .request("alpha.ping", None)
            .expect("second ping again")["pong"],
        serde_json::json!(true)
    );
    second.shutdown().expect("shutdown second core");
}
