//! #25: model-preflight env fixtures must join the crate-wide host env lock.
#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Source guard is necessary because separate private locks can pass in isolation.
use std::path::Path;

fn model_env_mutators(root: &Path, files: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            model_env_mutators(&path, files);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            let source = std::fs::read_to_string(&path).unwrap();
            if ["CODEX_HOME", "OPENAI_BASE_URL"]
                .iter()
                .any(|key| source.contains(key))
                && ["std::env::set_var", "std::env::remove_var"]
                    .iter()
                    .any(|operation| source.contains(operation))
            {
                files.push(path);
            }
        }
    }
}

fn uses_shared_lock(source: &str) -> bool {
    source.contains("crate::test_support::host_env_serial()")
        && !source.contains("static CODEX_ENV_LOCK")
}

#[test]
fn must_pass_model_env_mutators_use_shared_host_lock_25() {
    let mut files = Vec::new();
    model_env_mutators(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut files,
    );
    assert!(
        files
            .iter()
            .any(|path| path.ends_with("spawn_model_entry_25.rs")),
        "precondition: discover the actual model-preflight fixture"
    );
    for path in files {
        let source = std::fs::read_to_string(&path).unwrap();
        assert!(
            uses_shared_lock(&source),
            "{} mutates model env without the shared host_env_serial guard",
            path.display()
        );
    }
}

#[test]
fn must_fail_private_model_env_lock_does_not_satisfy_shared_lock_contract_25() {
    let source = "static CODEX_ENV_LOCK: Mutex<()> = Mutex::new(());";
    assert!(!uses_shared_lock(source));
    // Mentioning the shared lock must not conceal an independently locked fixture.
    let mixed = format!("{source}\nlet guard = crate::test_support::host_env_serial();");
    assert!(!uses_shared_lock(&mixed));
    assert!(uses_shared_lock(
        "let guard = crate::test_support::host_env_serial();"
    ));
    assert_eq!(source, "static CODEX_ENV_LOCK: Mutex<()> = Mutex::new(());");
}
