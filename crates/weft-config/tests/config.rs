use std::path::PathBuf;

use serde as _;
use thiserror as _;
use toml as _;
use weft_config::{Config, Error};

fn temporary(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("weft-config-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir.join("config.toml")
}

#[test]
fn a_missing_file_yields_the_defaults() {
    let path = temporary("missing");
    let config = Config::load(&path).expect("missing file is not an error");
    assert_eq!(config, Config::default());
    assert_eq!(config.terminal.scrollback, 10_000);
    assert!(!config.agent.auto_approve);
}

#[test]
fn partial_sections_fall_back_to_defaults() {
    let path = temporary("partial");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "[terminal]\nscrollback = 500\n").unwrap();

    let config = Config::load(&path).expect("partial file must load");
    assert_eq!(config.terminal.scrollback, 500);
    assert_eq!(config.terminal.theme, "weft-dark");
    assert_eq!(config.editor.history_limit, 5_000);
}

#[test]
fn unknown_keys_are_rejected_instead_of_ignored() {
    let path = temporary("unknown");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "[terminal]\nscrollbak = 500\n").unwrap();

    match Config::load(&path) {
        Err(Error::Parse { .. }) => {}
        other => panic!("expected a parse error, got {other:?}"),
    }
}

#[test]
fn values_out_of_range_are_rejected() {
    let path = temporary("range");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "[terminal]\nscrollback = 3\n").unwrap();

    match Config::load(&path) {
        Err(Error::Invalid { field, .. }) => assert_eq!(field, "terminal.scrollback"),
        other => panic!("expected an invalid value error, got {other:?}"),
    }
}

#[test]
fn an_empty_theme_is_rejected() {
    let path = temporary("theme");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "[terminal]\ntheme = \"\"\n").unwrap();

    assert!(matches!(
        Config::load(&path),
        Err(Error::Invalid {
            field: "terminal.theme",
            ..
        })
    ));
}

#[test]
fn saving_and_loading_round_trips_every_field() {
    let path = temporary("roundtrip");
    let config = Config {
        terminal: weft_config::Terminal {
            shell: Some("/bin/zsh".to_owned()),
            scrollback: 42_000,
            theme: "weft-light".to_owned(),
        },
        editor: weft_config::Editor {
            multiline: false,
            history_limit: 900,
        },
        agent: weft_config::Agent {
            provider: Some("openai".to_owned()),
            model: Some("gpt-4.1".to_owned()),
            base_url: Some("https://example.test/v1".to_owned()),
            auto_approve: true,
        },
        workflows: vec![weft_config::Workflow {
            name: "build".to_owned(),
            description: "compile everything".to_owned(),
            command: "cargo build {{profile}}".to_owned(),
        }],
    };

    config.save(&path).expect("save must succeed");
    let loaded = Config::load(&path).expect("load must succeed");
    assert_eq!(loaded, config);
}

#[test]
fn saving_creates_missing_directories() {
    let path = temporary("nested");
    let nested = path.parent().unwrap().join("a/b/c/config.toml");
    Config::default().save(&nested).expect("save must succeed");
    assert!(nested.exists());
}

#[test]
fn saving_replaces_the_previous_file_atomically() {
    let path = temporary("replace");
    let mut config = Config::default();
    config.terminal.scrollback = 1_000;
    config.save(&path).unwrap();
    config.terminal.scrollback = 2_000;
    config.save(&path).unwrap();

    assert_eq!(Config::load(&path).unwrap().terminal.scrollback, 2_000);
    assert!(!path.with_extension("toml.tmp").exists());
}

#[test]
fn workflows_load_with_parameters() {
    let path = temporary("workflows");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        "[[workflows]]\nname = \"greet\"\ndescription = \"say hi\"\ncommand = \"echo {{who}} {{who}}\"\n",
    )
    .unwrap();

    let config = Config::load(&path).expect("workflow must load");
    let workflow = &config.workflows[0];
    assert_eq!(workflow.name, "greet");
    assert_eq!(workflow.placeholders(), ["who"]);
    assert_eq!(
        workflow.expand(&[("who", "world")]).unwrap(),
        "echo world world"
    );
}

#[test]
fn a_missing_parameter_is_an_error() {
    let mut config = Config::default();
    config.workflows.push(weft_config::Workflow {
        name: "deploy".to_owned(),
        description: String::new(),
        command: "deploy {{target}}".to_owned(),
    });
    config.validate().unwrap();

    match config.workflows[0].expand(&[]) {
        Err(Error::MissingParameter {
            workflow,
            parameter,
        }) => {
            assert_eq!(workflow, "deploy");
            assert_eq!(parameter, "target");
        }
        other => panic!("expected a missing parameter error, got {other:?}"),
    }
}

#[test]
fn duplicate_workflow_names_are_rejected() {
    let mut config = Config::default();
    for _ in 0..2 {
        config.workflows.push(weft_config::Workflow {
            name: "build".to_owned(),
            description: String::new(),
            command: "cargo build".to_owned(),
        });
    }

    match config.validate() {
        Err(Error::Invalid { field, .. }) => assert_eq!(field, "workflows"),
        other => panic!("expected an invalid workflow error, got {other:?}"),
    }
}

#[test]
fn a_workflow_without_a_command_is_rejected() {
    let mut config = Config::default();
    config.workflows.push(weft_config::Workflow {
        name: "empty".to_owned(),
        description: String::new(),
        command: "   ".to_owned(),
    });

    match config.validate() {
        Err(Error::Invalid { field, .. }) => assert_eq!(field, "workflows"),
        other => panic!("expected an invalid workflow error, got {other:?}"),
    }
}
