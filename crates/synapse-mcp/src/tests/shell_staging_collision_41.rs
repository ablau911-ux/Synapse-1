//! #41 NEEDS-SEAM: thread-scoped path injection into the real staging allocator.
use super::*;

#[test]
fn must_pass_writer_retries_collision_preserving_old_staging_bytes_41() {
    let fx = fixture();
    let collision = fx.temp.path().join("status.json.old.tmp");
    let free = fx.temp.path().join("status.json.free.tmp");
    std::fs::write(&collision, b"earlier-generation").unwrap();
    assert_eq!(std::fs::read(&collision).unwrap(), b"earlier-generation");
    status_commit::with_staging_paths_for_test(&[collision.clone(), free.clone()], || {
        write_shell_job_status(&fx.paths.status_path, &terminal(&fx)).unwrap();
    });
    assert_eq!(committed(&fx.paths).status, "ok");
    assert_eq!(std::fs::read(collision).unwrap(), b"earlier-generation");
    assert!(!free.exists());
}

#[test]
fn must_fail_eight_staging_collisions_leave_committed_status_41() {
    let fx = fixture();
    write(&fx.paths, &fx.status);
    let before = std::fs::read(&fx.paths.status_path).unwrap();
    let collision = fx.temp.path().join("status.json.old.tmp");
    std::fs::write(&collision, b"earlier-generation").unwrap();
    let paths = vec![collision.clone(); 8];
    let error = status_commit::with_staging_paths_for_test(&paths, || {
        write_shell_job_status(&fx.paths.status_path, &terminal(&fx))
    })
    .expect_err("bounded collision refusal");
    assert_eq!(
        error.data.as_ref().unwrap()["code"],
        error_codes::STORAGE_WRITE_FAILED
    );
    assert_eq!(
        error.data.as_ref().unwrap()["reason"],
        "job_status_temp_create_failed"
    );
    assert_eq!(std::fs::read(&fx.paths.status_path).unwrap(), before);
    assert_eq!(std::fs::read(collision).unwrap(), b"earlier-generation");
}
