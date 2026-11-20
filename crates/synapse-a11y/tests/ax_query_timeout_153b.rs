//! #153 part 2: `Accessibility.queryAXTree` hangs on occluded/minimized
//! windows; the call must be bounded and fail fast with a precise hint.

use std::time::Duration;

use synapse_a11y::{A11yError, bound_ax_query};

#[tokio::test]
async fn must_fail_hung_ax_query_times_out_with_occlusion_hint() {
    let hung = std::future::pending::<Result<u32, String>>();
    let started = std::time::Instant::now();
    let result = bound_ax_query(Duration::from_millis(50), "role=\"button\"", hung).await;
    assert!(started.elapsed() < Duration::from_secs(5));
    match result {
        Err(A11yError::CdpAxtreeFailed { detail }) => {
            assert!(detail.contains("timed out after 50 ms"), "{detail}");
            assert!(detail.contains("occluded"), "{detail}");
            assert!(detail.contains("un-minimize"), "{detail}");
        }
        other => panic!("expected CdpAxtreeFailed timeout, got {other:?}"),
    }
}

#[tokio::test]
async fn must_pass_fast_ax_query_passes_through_value_and_error() {
    let ok = bound_ax_query(Duration::from_secs(1), "q", async {
        Ok::<_, String>(7_u32)
    })
    .await;
    assert!(matches!(ok, Ok(7)));
    let err = bound_ax_query(Duration::from_secs(1), "q", async {
        Err::<u32, _>("boom".to_owned())
    })
    .await;
    match err {
        Err(A11yError::CdpAxtreeFailed { detail }) => assert!(detail.contains("boom"), "{detail}"),
        other => panic!("expected CdpAxtreeFailed, got {other:?}"),
    }
}
