macro_rules! assert_cf_literal {
    ($name:ident) => {
        assert_eq!(synapse_storage::cf::$name, stringify!($name));
    };
}

#[test]
fn must_pass_cf_constants_match_literal_names() {
    assert_cf_literal!(CF_EVENTS);
    assert_cf_literal!(CF_OBSERVATIONS);
    assert_cf_literal!(CF_PROFILES);
    assert_cf_literal!(CF_MODEL_CACHE);
    assert_cf_literal!(CF_SESSIONS);
    assert_cf_literal!(CF_REFLEX_AUDIT);
    assert_cf_literal!(CF_OCR_CACHE);
    assert_cf_literal!(CF_TELEMETRY);
    assert_cf_literal!(CF_ACTION_LOG);
    assert_cf_literal!(CF_PROCESS_HISTORY);
    assert_cf_literal!(CF_KV);
    assert_cf_literal!(CF_TIMELINE);
    assert_cf_literal!(CF_EPISODES);
    assert_cf_literal!(CF_ROUTINES);
    assert_cf_literal!(CF_ROUTINE_STATE);
    assert_cf_literal!(CF_AGENT_EVENTS);
    assert_cf_literal!(CF_AGENT_TRANSCRIPTS);
}

/// The declared column-family set is the PRD §4 list (17 CFs, no implicit
/// `default`). Reads the real `cf::ALL_COLUMN_FAMILIES`, so adding, dropping,
/// renaming or duplicating a CF fails here.
#[test]
fn must_pass_cf_names_sorted_snapshot() {
    let mut actual = synapse_storage::cf::ALL_COLUMN_FAMILIES.to_vec();
    actual.sort_unstable();
    let declared_len = actual.len();
    actual.dedup();
    assert_eq!(actual.len(), declared_len, "duplicate CF name declared");
    let expected = [
        "CF_ACTION_LOG",
        "CF_AGENT_EVENTS",
        "CF_AGENT_TRANSCRIPTS",
        "CF_EPISODES",
        "CF_EVENTS",
        "CF_KV",
        "CF_MODEL_CACHE",
        "CF_OBSERVATIONS",
        "CF_OCR_CACHE",
        "CF_PROCESS_HISTORY",
        "CF_PROFILES",
        "CF_REFLEX_AUDIT",
        "CF_ROUTINES",
        "CF_ROUTINE_STATE",
        "CF_SESSIONS",
        "CF_TELEMETRY",
        "CF_TIMELINE",
    ];
    assert_eq!(actual, expected);
    assert!(
        !synapse_storage::cf::ALL_COLUMN_FAMILIES.contains(&"default"),
        "RocksDB implicit default CF must not be declared"
    );
}

#[test]
fn must_fail_undeclared_or_miscased_cf_names_are_refused_without_side_effects()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let db = synapse_storage::Db::open(&temp.path().join("db"), 1)?;
    for name in ["", "cf_kv", "CF_KV ", "CF_NOT_DECLARED"] {
        assert!(
            !synapse_storage::cf::ALL_COLUMN_FAMILIES.contains(&name),
            "{name:?} must not be a declared column family"
        );
        let result = db.put_batch(name, [(b"probe".to_vec(), b"x".to_vec())]);
        match result {
            Ok(()) => panic!("write to undeclared CF {name:?} must be refused"),
            Err(error) => assert_eq!(
                error.code(),
                synapse_core::error_codes::STORAGE_WRITE_FAILED,
                "{name:?}: {error}"
            ),
        }
    }
    assert!(db.scan_cf(synapse_storage::cf::CF_KV)?.is_empty());
    Ok(())
}
