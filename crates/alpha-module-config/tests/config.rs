use alpha_module_config::{ConfigSource, ConfigStack};
use serde::Deserialize;

fn table(source: &str) -> toml::Table {
    source.parse().expect("valid toml")
}

#[test]
fn later_layers_override_earlier_layers() {
    let mut stack = ConfigStack::new();
    stack.push(ConfigSource::Defaults, table("a = 1\nb = 1"));
    stack.push(ConfigSource::Project, table("a = 2"));
    stack.push(ConfigSource::Cli, table("a = 3"));

    let resolved = stack.resolve();
    assert_eq!(resolved.get("a").and_then(toml::Value::as_integer), Some(3));
    assert_eq!(resolved.get("b").and_then(toml::Value::as_integer), Some(1));
    assert_eq!(resolved.source_of("a"), Some(ConfigSource::Cli));
    assert_eq!(resolved.source_of("b"), Some(ConfigSource::Defaults));
}

#[test]
fn nested_tables_merge_and_keep_provenance() {
    let mut stack = ConfigStack::new();
    stack.push(
        ConfigSource::User,
        table("[server]\nhost = \"localhost\"\nport = 1"),
    );
    stack.push(ConfigSource::Project, table("[server]\nport = 2"));

    let resolved = stack.resolve();
    assert_eq!(
        resolved.get("server.host").and_then(toml::Value::as_str),
        Some("localhost")
    );
    assert_eq!(
        resolved
            .get("server.port")
            .and_then(toml::Value::as_integer),
        Some(2)
    );
    assert_eq!(resolved.source_of("server.host"), Some(ConfigSource::User));
    assert_eq!(
        resolved.source_of("server.port"),
        Some(ConfigSource::Project)
    );
}

#[test]
fn replacing_a_table_clears_old_provenance() {
    let mut stack = ConfigStack::new();
    stack.push(ConfigSource::User, table("[server]\nhost = \"localhost\""));
    stack.push(ConfigSource::Project, table("server = \"off\""));

    let resolved = stack.resolve();
    assert_eq!(
        resolved.get("server").and_then(toml::Value::as_str),
        Some("off")
    );
    assert_eq!(resolved.source_of("server"), Some(ConfigSource::Project));
    assert_eq!(resolved.source_of("server.host"), None);
}

#[derive(Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct Sample {
    name: String,
    retries: u32,
}

#[test]
fn decode_accepts_valid_tables_and_rejects_unknown_keys() {
    let mut stack = ConfigStack::new();
    stack.push(
        ConfigSource::Defaults,
        table("name = \"alpha\"\nretries = 3"),
    );
    let resolved = stack.resolve();
    assert_eq!(
        resolved.decode::<Sample>().expect("decode"),
        Sample {
            name: "alpha".to_owned(),
            retries: 3,
        }
    );

    let mut stack = ConfigStack::new();
    stack.push(
        ConfigSource::Defaults,
        table("name = \"alpha\"\nretries = 3\nextra = true"),
    );
    assert!(stack.resolve().decode::<Sample>().is_err());
}
