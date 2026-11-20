//! Regression for #223: the `user_activity` source_of_truth must describe how
//! the response is actually produced, not claim a foreground correlation.

use super::{Parameters, SOURCE_OF_TRUTH, SynapseService, UserActivityParams};

/// The exact contract text (#223): hook event re-read by HWND, a separate live
/// foreground readback, and per-window DPI. Any rewording, including one that
/// reintroduces a foreground-correlation claim, must be a deliberate change.
const EXPECTED_SOURCE_OF_TRUTH: &str = "physical Windows keyboard/mouse hook event (user_window re-read by HWND) + live GetForegroundWindow readback (current_foreground); dpi_scale per window from GetDpiForWindow";

#[test]
fn must_fail_source_of_truth_does_not_claim_foreground_correlation() {
    assert_eq!(SOURCE_OF_TRUTH, EXPECTED_SOURCE_OF_TRUTH);
    assert!(
        !SOURCE_OF_TRUTH.contains("correlated with foreground window"),
        "{SOURCE_OF_TRUTH}"
    );
}

#[test]
fn must_pass_source_of_truth_names_the_hwnd_reread_and_window_dpi_sources() {
    assert!(
        SOURCE_OF_TRUTH.contains("GetForegroundWindow"),
        "{SOURCE_OF_TRUTH}"
    );
    assert!(
        SOURCE_OF_TRUTH.contains("GetDpiForWindow"),
        "{SOURCE_OF_TRUTH}"
    );
}

/// The real tool response carries that exact source_of_truth (works with or
/// without a foreground window, e.g. on a locked screen).
#[tokio::test]
async fn must_pass_user_activity_response_carries_the_exact_source_of_truth() -> anyhow::Result<()>
{
    let root = tempfile::tempdir()?;
    let service = SynapseService::try_new_isolated_for_tests(root.path())?;

    let response = service
        .user_activity(Parameters(UserActivityParams::default()))
        .await
        .map_err(|error| anyhow::anyhow!("user_activity failed: {error:?}"))?
        .0;

    assert_eq!(response.source_of_truth, EXPECTED_SOURCE_OF_TRUTH);
    Ok(())
}
