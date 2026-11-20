#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::mem::size_of;

use synapse_core::Rect;
use synapse_livefeed::history::{ChangeRecord, History};

fn record(seq: u64, ts_ms: u64, rectangles: usize) -> ChangeRecord {
    ChangeRecord {
        seq,
        ts_ms,
        width: 2,
        height: 2,
        rects: vec![
            Rect {
                x: 0,
                y: 0,
                w: 1,
                h: 1
            };
            rectangles
        ],
    }
}

#[test]
fn must_pass_byte_cap_evicts_oldest() {
    let cap = 2 * size_of::<ChangeRecord>() + 3 * size_of::<Rect>();
    let mut history = History::new(120_000, cap);
    history.push(record(1, 100, 1));
    history.push(record(2, 101, 2));
    assert_eq!(history.bytes(), cap, "count both record structs and rects");
    assert_eq!(history.oldest_seq(), Some(1), "exact cap retains oldest");
    let after_first = history.records_after(1).expect("first cursor retained");
    assert_eq!(after_first.len(), 1);
    assert_eq!(after_first[0].seq, 2);
    assert_eq!(after_first[0].rects.len(), 2);

    history.push(record(3, 102, 1));
    assert_eq!(history.bytes(), cap);
    assert_eq!(history.oldest_seq(), Some(2));
    assert!(history.records_after(1).is_none(), "old cursor was evicted");
    let after_second = history.records_after(2).expect("newer cursor retained");
    assert_eq!(after_second.len(), 1);
    assert_eq!(after_second[0].seq, 3);
    assert!(history.records_after(3).expect("latest cursor").is_empty());
}

#[test]
fn must_pass_time_cap_evicts_old() {
    let mut history = History::new(100, 1_024);
    history.push(record(1, 1_000, 1));
    history.push(record(2, 1_050, 1));
    assert_eq!(history.oldest_seq(), Some(1));
    history.push(record(3, 1_101, 1));
    assert_eq!(history.oldest_seq(), Some(2), "101 ms exceeds 100 ms");
    assert!(history.records_after(1).is_none());
    let records = history.records_after(2).expect("51 ms cursor stays");
    assert_eq!(records.len(), 1);
    assert_eq!((records[0].seq, records[0].ts_ms), (3, 1_101));
    assert_eq!(
        history.bytes(),
        2 * (size_of::<ChangeRecord>() + size_of::<Rect>())
    );
}

#[test]
fn must_pass_time_eviction_runs_without_new_changes() {
    let mut history = History::new(100, 1_024);
    history.push(record(1, 1_000, 1));
    history.push(record(2, 1_050, 1));
    history.evict(1_100);
    assert_eq!(history.oldest_seq(), Some(1), "exact age boundary stays");
    history.evict(1_101);
    assert_eq!(history.oldest_seq(), Some(2));
    assert!(history.records_after(1).is_none());
    assert_eq!(
        history.bytes(),
        size_of::<ChangeRecord>() + size_of::<Rect>()
    );
    history.evict(1_151);
    assert_eq!(history.oldest_seq(), None);
    assert_eq!(history.bytes(), 0, "idle history releases all record bytes");
    assert!(history.records_after(2).is_none());
}
