//! #270/#272: OCR-cache pressure skips persistence with a STORAGE_WRITE_SHED
//! notice while preserving successful OCR, never STORAGE_WRITE_FAILED.
//!
//! SOURCE GUARD, not a behavioural test: the gate sits inside
//! `persist_ocr_cache_row`. Behavioural coverage lives in
//! `ocr_cache_best_effort_272.rs`; the manager's FSV checks real `read_text`
//! success, the cache-skip notice and an independent cache readback.

const SOURCE: &str = include_str!("../m1_tools.rs");

#[test]
fn must_pass_ocr_cache_pressure_gate_notes_shed_without_write_270() {
    let source = SOURCE.replace("\r\n", "\n");
    let gate = source
        .find("storage_pressure_permits_write(cf::CF_OCR_CACHE)")
        .expect("the OCR cache pressure gate exists");
    let end = source[gate..]
        .find("\n    }")
        .expect("pressure branch ends before cache persistence");
    let block = &source[gate..gate + end];
    assert!(
        block.contains("note_ocr_cache_shed(&mut row.result, &row.cache_key);")
            && block.contains("return Ok(false);"),
        "#272: cache shedding must annotate successful OCR and skip persistence: {block}"
    );
    assert!(
        !block.contains("Err(")
            && !block.contains("STORAGE_WRITE_FAILED")
            && !block.contains("storage_put_rows("),
        "#272: cache shedding must not refuse successful OCR: {block}"
    );
    let note = source
        .split_once("fn note_ocr_cache_shed(")
        .expect("cache-skip notice helper exists")
        .1
        .split("\nfn ")
        .next()
        .expect("cache-skip notice body");
    assert!(
        note.contains("error_codes::STORAGE_WRITE_SHED")
            && note.contains("OCR cache write skipped")
            && note.contains("result.cache_write = Some(synapse_core::OcrCacheWriteNotice {")
            && note.contains("code: error_codes::STORAGE_WRITE_SHED.to_owned()")
            && note.contains("detail: note")
            && !note.contains("STORAGE_WRITE_FAILED")
            && !note.contains("storage_put_rows(")
            && !note.contains("Err("),
        "#272: shedding must carry STORAGE_WRITE_SHED in a non-error notice without writing: {note}"
    );
}
