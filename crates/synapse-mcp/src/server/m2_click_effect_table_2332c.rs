//! #2332 frozen behaviour table C (manager-owned, work method v3). Coders never
//! edit this file. Point picture continuity: the before / after pictures at the
//! click point prove "changed" only when both were taken at the same place
//! (pre-release review R-TRUTH / E2: a window that moved between two individually
//! stable reads must not read as a click effect).

use synapse_core::Rect;

use super::super::ClickPixelSignature;
use super::{PointPixelEvidence, point_pixel_evidence};

const AT_POINT: Rect = Rect {
    x: 390,
    y: 290,
    w: 20,
    h: 20,
};
/// The same box after the window moved 40 px right and 25 px down.
const MOVED: Rect = Rect {
    x: 430,
    y: 315,
    w: 20,
    h: 20,
};

fn picture(status: &str, region: Rect, hash: Option<&str>) -> Option<ClickPixelSignature> {
    Some(ClickPixelSignature {
        status: status.to_owned(),
        region,
        bitmap_sha256: hash.map(ToOwned::to_owned),
        detail: Some("synthetic point picture".to_owned()),
    })
}

fn ok(region: Rect, hash: &str) -> Option<ClickPixelSignature> {
    picture("ok", region, Some(hash))
}

fn unavailable(region: Rect) -> Option<ClickPixelSignature> {
    picture("unavailable", region, None)
}

#[test]
fn point_picture_continuity_table_2332c() {
    #[allow(clippy::type_complexity)]
    let rows: [(
        &str,
        Option<ClickPixelSignature>,
        Option<ClickPixelSignature>,
        PointPixelEvidence,
    ); 10] = [
        (
            "X1_same_region_different_hashes",
            ok(AT_POINT, "hash-before"),
            ok(AT_POINT, "hash-after"),
            PointPixelEvidence::Changed,
        ),
        (
            "X2_same_region_same_hash",
            ok(AT_POINT, "hash-same"),
            ok(AT_POINT, "hash-same"),
            PointPixelEvidence::Unchanged,
        ),
        (
            "X3_window_moved_between_reads_different_hashes",
            ok(AT_POINT, "hash-before"),
            ok(MOVED, "hash-after"),
            PointPixelEvidence::Unavailable,
        ),
        (
            "X4_window_moved_between_reads_same_hash",
            ok(AT_POINT, "hash-same"),
            ok(MOVED, "hash-same"),
            PointPixelEvidence::Unavailable,
        ),
        (
            "X5_after_not_ok_same_region",
            ok(AT_POINT, "hash-before"),
            unavailable(AT_POINT),
            PointPixelEvidence::Unavailable,
        ),
        (
            "X5_before_not_ok_regions_differ",
            unavailable(AT_POINT),
            ok(MOVED, "hash-after"),
            PointPixelEvidence::Unavailable,
        ),
        (
            "X5_both_not_ok",
            unavailable(AT_POINT),
            unavailable(MOVED),
            PointPixelEvidence::Unavailable,
        ),
        (
            "X6_after_missing",
            ok(AT_POINT, "hash-before"),
            None,
            PointPixelEvidence::Unavailable,
        ),
        (
            "X6_before_missing",
            None,
            ok(AT_POINT, "hash-after"),
            PointPixelEvidence::Unavailable,
        ),
        (
            "X6_both_missing",
            None,
            None,
            PointPixelEvidence::Unavailable,
        ),
    ];
    let mut failures = Vec::new();
    for (row, before, after, expected) in &rows {
        let got = point_pixel_evidence(before.as_ref(), after.as_ref());
        if got != *expected {
            failures.push(format!(
                "row {row}: evidence {got:?}, expected {expected:?}"
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "#2332c point picture continuity rows failed:\n{}",
        failures.join("\n")
    );
}
