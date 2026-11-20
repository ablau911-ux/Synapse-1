//! criteria_b1.md:8. Mutants run through the same scanner as the real inventory.
use super::write_mapper_inventory;

#[test]
fn must_fail_named_mapper_hardcoded_write_failed_274() {
    let source = r#"
fn persist(db: &Db) {
    db.put_batch(CF_KV, rows).map_err(storage_error)?;
}
fn storage_error(error: StorageError) -> ErrorData {
    mcp_error(error_codes::STORAGE_WRITE_FAILED, error.to_string())
}
"#;
    let (sites, wrong) = write_mapper_inventory(source);
    assert_eq!(sites, 1, "must discover the real write call");
    assert_eq!(
        wrong,
        vec![3],
        "named mapper hides WriteShed; flag the write site"
    );
}

#[test]
fn must_fail_named_mapper_hardcoded_unrelated_code_274() {
    let source = r#"
fn storage_error(error: StorageError) -> ErrorData {
    mcp_error(error_codes::REFLEX_PARAMS_INVALID, error.to_string())
}
fn persist(db: &Db) {
    db.storage_put_json(CF_KV, key, row).map_err(storage_error)?;
}
"#;
    let (sites, wrong) = write_mapper_inventory(source);
    assert_eq!(sites, 1);
    assert_eq!(
        wrong,
        vec![6],
        "mapper replacing the storage code must fail"
    );
}

#[test]
fn named_mapper_wrapping_original_error_preserves_code_274() {
    for body in [
        "Box::new(error)",
        "Error::from(error)",
        "error.into()",
        "Self::Storage(error)",
        "return Box::new(error);",
    ] {
        let source = format!(
            "fn persist(db: &Db) {{\n    db.put_batch(CF_KV, rows).map_err(session_store_error)?;\n}}\n\
             fn session_store_error(error: synapse_storage::StorageError) -> SessionStoreError {{ {body} }}"
        );
        let (sites, wrong) = write_mapper_inventory(&source);
        assert_eq!(sites, 1, "must discover the write for {body}");
        assert!(
            wrong.is_empty(),
            "pass-through mapper flagged: {body}: {wrong:?}"
        );
    }
}

#[test]
fn must_fail_named_mapper_wrapping_replacement_or_ignoring_error_274() {
    for body in [
        "Box::new(replacement)",
        "Error::from(error_codes::STORAGE_WRITE_FAILED)",
        "Self::Storage(StorageError::WriteFailed)",
        "replacement.into()",
        "let error = replacement; Box::new(error)",
        "Box::new(error); Error::from(error_codes::STORAGE_WRITE_FAILED)",
    ] {
        let source = format!(
            "fn storage_error(error: StorageError) -> ErrorData {{ {body} }}\n\
             fn persist(db: &Db) {{\n    db.put_batch(CF_KV, rows).map_err(storage_error)?;\n}}"
        );
        let (sites, wrong) = write_mapper_inventory(&source);
        assert_eq!(sites, 1);
        assert_eq!(
            wrong,
            vec![3],
            "replacement mapper escaped detection: {body}"
        );
    }
}
