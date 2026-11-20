//! #216 variant: a corrupt reality head cannot abort another profile's scan.
use super::tests::{
    install_synthetic_input, service_with_profile_dir, synthetic_input, write_profile,
};
use super::*;
use std::{
    io::Write,
    sync::{Arc, Mutex},
};

#[derive(Clone, Default)]
struct Warnings(Arc<Mutex<Vec<u8>>>);
impl Write for Warnings {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .expect("warning buffer")
            .extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

async fn fixture() -> (tempfile::TempDir, SynapseService, RealityHeadRow) {
    let dir = tempfile::tempdir().expect("directory");
    let profiles = dir.path().join("profiles");
    std::fs::create_dir_all(&profiles).expect("profiles");
    write_profile(
        &profiles.join("synthetic.toml"),
        "synthetic",
        "single_player",
    )
    .expect("profile");
    let svc = service_with_profile_dir(dir.path(), &profiles).expect("service");
    install_synthetic_input(&svc, synthetic_input("Synthetic Window")).expect("input");
    let head = svc
        .reality_baseline(Parameters(RealityBaselineParams {
            profile_id: Some("synthetic".into()),
            epoch_id: Some("healthy216".into()),
            force_new_epoch: true,
            include: Vec::new(),
            depth: DEFAULT_DEPTH,
            max_elements: DEFAULT_MAX_ELEMENTS,
        }))
        .await
        .expect("baseline")
        .0
        .head;
    (dir, svc, head)
}

fn stored(svc: &SynapseService, key: &str) -> Option<Vec<u8>> {
    svc.reflex_runtime()
        .expect("runtime")
        .lock()
        .expect("runtime lock")
        .storage_kv_row(key.as_bytes())
        .expect("physical row read")
}

#[tokio::test]
async fn must_pass_healthy_reality_head_remains_findable_by_epoch() {
    let (_dir, svc, head) = fixture().await;
    let key = head_key(&head.profile_key);
    let before = stored(&svc, &key).expect("baseline row");
    let found = svc
        .find_reality_head_by_epoch(&head.epoch_id, "other-profile")
        .expect("scan")
        .expect("healthy head");
    assert_eq!(found.profile_key, head.profile_key);
    assert_eq!(found.epoch_id, head.epoch_id);
    assert_eq!(stored(&svc, &key), Some(before));
}

#[tokio::test]
async fn must_fail_corrupt_head_cannot_abort_scan_or_repeat_warning_or_change_rows() {
    let (_dir, svc, head) = fixture().await;
    // Unique key avoids the process-global warning set shared by other tests.
    let key = head_key(&format!("a-corrupt216-{}", new_epoch_id()));
    let bad = b"{invalid-head-216".to_vec();
    svc.reflex_runtime()
        .expect("runtime")
        .lock()
        .expect("runtime lock")
        .storage_put_kv_rows(vec![(key.as_bytes().to_vec(), bad.clone())])
        .expect("seed corrupt row");
    assert_eq!(stored(&svc, &key), Some(bad.clone()));
    let healthy_key = head_key(&head.profile_key);
    let healthy = stored(&svc, &healthy_key).expect("healthy precondition");
    let output = Warnings::default();
    let writer = output.clone();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_max_level(tracing::Level::WARN)
        .with_writer(move || writer.clone())
        .finish();
    tracing::subscriber::with_default(subscriber, || {
        for _ in 0..2 {
            let found = svc
                .find_reality_head_by_epoch(&head.epoch_id, "other-profile")
                .expect("skip corrupt row")
                .expect("healthy row after corrupt row");
            assert_eq!(found.epoch_id, head.epoch_id);
            assert!(matches!(
                svc.lookup_reality_head(key.strip_prefix(&head_key("")).expect("prefix"))
                    .expect("lookup"),
                HeadLookup::Undecodable
            ));
        }
    });
    let log = String::from_utf8(output.0.lock().expect("captured warnings").clone()).expect("UTF8");
    assert_eq!(
        log.matches("REALITY_HEAD_ROW_UNDECODABLE").count(),
        1,
        "{log}"
    );
    assert!(
        log.contains(&key),
        "warning identifies physical corrupt row: {log}"
    );
    assert_eq!(stored(&svc, &key), Some(bad));
    assert_eq!(stored(&svc, &healthy_key), Some(healthy));
}
