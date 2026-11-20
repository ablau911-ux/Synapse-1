//! Supplemental guard; behavior proof is video_cache_digest_174, not this scan.
#[test]
fn must_pass_video_pixel_integrity_hashes_remain_plain_174() -> std::io::Result<()> {
    let source = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/server/videoanalyze.rs"),
    )?;
    assert!(
        source.contains("Sha256::digest(&bgra)"),
        "frame integrity contract"
    );
    assert!(
        source.contains("Sha256::digest(&output.stdout)"),
        "PNG integrity contract"
    );
    Ok(())
}

#[test]
fn must_fail_video_source_identity_never_uses_plain_sha256_174() -> std::io::Result<()> {
    let source = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/server/videoanalyze.rs"),
    )?;
    let compact: String = source
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .flat_map(str::chars)
        .filter(|ch| !ch.is_whitespace())
        .collect();
    assert!(
        !compact.contains("Sha256::digest(source.as_bytes())"),
        "offline dictionary-checkable URL cache identity (#174)"
    );
    Ok(())
}
