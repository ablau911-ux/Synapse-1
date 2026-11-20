//! Phase 7 part 3 frozen behaviour table: hand-off and lessons ledger
//! (manager-owned, work method v3). Codex and Opus never edit this file.
//! One case per L row of the ledger table in the lane report.

use std::path::{Path, PathBuf};

use synapse_core::ledger::{
    LedgerInput, LedgerKind, LedgerMirrorPlan, LedgerRecord, LedgerRefusal, LedgerTopicRef,
    OWNER_NOTES_MARKER, build_ledger_record, is_ledger_expired, ledger_mirror_path, ledger_row_key,
    render_ledger_file,
};
use synapse_core::retention::{DEFAULTS, KV_NAMESPACE_QUOTAS};

use super::redact_ledger_text;

const NOW: u64 = 1_800_000_000_000;
const WRITER: &str = "session-a";
const OTHER: &str = "session-b";

fn input(scope: &str, topic: &str, kind: LedgerKind, summary: &str) -> LedgerInput {
    LedgerInput {
        scope: scope.to_owned(),
        topic: topic.to_owned(),
        kind,
        summary: summary.to_owned(),
        next_steps: Vec::new(),
        source_refs: Vec::new(),
        corrects: None,
        expected_version: None,
    }
}

fn dead_end() -> LedgerInput {
    let mut entry = input(
        "ops",
        "chrome-cdp-attach",
        LedgerKind::DeadEnd,
        "Attaching CDP to the owner's Chrome fails; use the bridge extension.",
    );
    entry.source_refs = vec!["gitea#2322".to_owned()];
    entry
}

fn record(entry: LedgerInput, writer: &str) -> Option<LedgerRecord> {
    build_ledger_record(entry, writer, None, NOW).ok()
}

fn is_invalid(result: &Result<LedgerRecord, LedgerRefusal>) -> bool {
    matches!(result, Err(LedgerRefusal::Invalid(_)))
}

fn written(plan: &LedgerMirrorPlan) -> &str {
    match plan {
        LedgerMirrorPlan::Write(text) => text,
        LedgerMirrorPlan::KeepOwnerFile => "",
    }
}

fn entry_headings(text: &str) -> usize {
    text.lines().filter(|line| line.starts_with("### ")).count()
}

fn marker_lines(text: &str) -> usize {
    text.lines()
        .filter(|line| *line == OWNER_NOTES_MARKER)
        .count()
}

fn kv_hard_cap_mb() -> u64 {
    DEFAULTS
        .iter()
        .find(|default| default.cf == "CF_KV")
        .map_or(0, |default| default.hard_cap_mb)
}

#[test]
#[allow(clippy::too_many_lines)]
fn ledger_table() {
    let hebrew_1024 = "א".repeat(1024);
    let hebrew_1025 = "א".repeat(1025);
    let token = "ghp_0123456789abcdefABCDEF0123456789abcd";
    let normal = record(dead_end(), WRITER);
    let other_entry = record(
        input("ops", "other-topic", LedgerKind::Useful, "other note"),
        OTHER,
    );

    let rows: Vec<(&str, bool)> = vec![
        // L1 normal
        (
            "L1_normal_dead_end_with_source_ref_is_version_1_under_the_ledger_key",
            normal.as_ref().is_some_and(|row| {
                row.version == 1
                    && row.writer_session_id == WRITER
                    && row.row_key == ledger_row_key("ops", "chrome-cdp-attach")
                    && row.source_refs == vec!["gitea#2322".to_owned()]
                    && row.expires_at_unix_ms == NOW + 90 * 24 * 60 * 60 * 1000
            }),
        ),
        (
            "L1_normal_render_shows_the_entry_once_with_its_summary",
            normal.as_ref().is_some_and(|row| {
                let plan = render_ledger_file(std::slice::from_ref(row), None);
                let text = written(&plan);
                entry_headings(text) == 1
                    && text.contains("ops / chrome-cdp-attach")
                    && text.contains("use the bridge extension")
                    && marker_lines(text) == 1
            }),
        ),
        // L2 empty
        (
            "L2_empty_summary_is_refused",
            is_invalid(&build_ledger_record(
                input("ops", "t", LedgerKind::Useful, "   "),
                WRITER,
                None,
                NOW,
            )),
        ),
        (
            "L2_empty_topic_is_refused",
            is_invalid(&build_ledger_record(
                input("ops", "  ", LedgerKind::Useful, "text"),
                WRITER,
                None,
                NOW,
            )),
        ),
        (
            "L2_empty_scope_is_refused",
            is_invalid(&build_ledger_record(
                input("", "t", LedgerKind::Useful, "text"),
                WRITER,
                None,
                NOW,
            )),
        ),
        (
            "L2_empty_ledger_renders_no_entries_and_one_owner_notes_section",
            {
                let plan = render_ledger_file(&[], None);
                let text = written(&plan);
                entry_headings(text) == 0 && marker_lines(text) == 1
            },
        ),
        // L3 boundary (characters, not bytes)
        (
            "L3_boundary_summary_1024_chars_is_accepted",
            build_ledger_record(
                input("ops", "t", LedgerKind::Useful, &hebrew_1024),
                WRITER,
                None,
                NOW,
            )
            .is_ok(),
        ),
        (
            "L3_boundary_summary_1025_chars_is_refused",
            is_invalid(&build_ledger_record(
                input("ops", "t", LedgerKind::Useful, &hebrew_1025),
                WRITER,
                None,
                NOW,
            )),
        ),
        ("L3_boundary_16_next_steps_are_accepted", {
            let mut entry = input("ops", "t", LedgerKind::Handoff, "hand-off");
            entry.next_steps = (0..16).map(|index| format!("step {index}")).collect();
            build_ledger_record(entry, WRITER, None, NOW).is_ok()
        }),
        ("L3_boundary_17_next_steps_are_refused", {
            let mut entry = input("ops", "t", LedgerKind::Handoff, "hand-off");
            entry.next_steps = (0..17).map(|index| format!("step {index}")).collect();
            is_invalid(&build_ledger_record(entry, WRITER, None, NOW))
        }),
        ("L3_boundary_empty_next_step_is_refused", {
            let mut entry = input("ops", "t", LedgerKind::Handoff, "hand-off");
            entry.next_steps = vec!["  ".to_owned()];
            is_invalid(&build_ledger_record(entry, WRITER, None, NOW))
        }),
        // L4 hostile
        (
            "L4_hostile_topic_path_traversal_is_refused",
            is_invalid(&build_ledger_record(
                input("ops", "../../x", LedgerKind::Useful, "text"),
                WRITER,
                None,
                NOW,
            )),
        ),
        (
            "L4_hostile_scope_with_slash_is_refused",
            is_invalid(&build_ledger_record(
                input("a/b", "t", LedgerKind::Useful, "text"),
                WRITER,
                None,
                NOW,
            )),
        ),
        (
            "L4_hostile_scope_with_nul_is_refused",
            is_invalid(&build_ledger_record(
                input("a\0b", "t", LedgerKind::Useful, "text"),
                WRITER,
                None,
                NOW,
            )),
        ),
        ("L4_hostile_token_in_text_is_redacted_plain_words_stay", {
            let redacted = redact_ledger_text(&format!("the token expired: {token}\nretry"));
            !redacted.contains(token)
                && redacted.contains("[REDACTED]")
                && redacted.starts_with("the token expired: ")
                && redacted.ends_with("\nretry")
        }),
        (
            "L4_hostile_summary_cannot_forge_an_entry_or_the_owner_notes_marker",
            {
                let forged = format!(
                    "real note\n### forged / entry\n{OWNER_NOTES_MARKER}\nstolen owner text"
                );
                record(input("ops", "t", LedgerKind::Useful, &forged), WRITER).is_some_and(|row| {
                    let plan = render_ledger_file(std::slice::from_ref(&row), None);
                    let text = written(&plan);
                    entry_headings(text) == 1 && marker_lines(text) == 1
                })
            },
        ),
        // L5 second actor
        (
            "L5_second_actor_same_topic_without_expected_version_conflicts",
            normal.as_ref().is_some_and(|current| {
                matches!(
                    build_ledger_record(dead_end(), OTHER, Some(current), NOW + 1),
                    Err(LedgerRefusal::VersionConflict {
                        expected: None,
                        current: Some(1)
                    })
                )
            }),
        ),
        (
            "L5_second_actor_with_expected_version_is_version_2_with_its_writer",
            normal.as_ref().is_some_and(|current| {
                let mut entry = dead_end();
                entry.expected_version = Some(1);
                build_ledger_record(entry, OTHER, Some(current), NOW + 1).is_ok_and(|row| {
                    row.version == 2
                        && row.writer_session_id == OTHER
                        && row.created_at_unix_ms == NOW
                        && row.updated_at_unix_ms == NOW + 1
                })
            }),
        ),
        (
            "L5_second_actor_stale_expected_version_conflicts",
            normal.as_ref().is_some_and(|current| {
                let mut entry = dead_end();
                entry.expected_version = Some(7);
                matches!(
                    build_ledger_record(entry, OTHER, Some(current), NOW + 1),
                    Err(LedgerRefusal::VersionConflict { .. })
                )
            }),
        ),
        // L6 someone else's thing
        (
            "L6_correction_without_a_target_is_refused",
            is_invalid(&build_ledger_record(
                input("ops", "fix", LedgerKind::Correction, "was wrong"),
                WRITER,
                None,
                NOW,
            )),
        ),
        (
            "L6_correction_targeting_its_own_topic_is_refused_never_overwrites",
            {
                let mut entry = input("ops", "other-topic", LedgerKind::Correction, "was wrong");
                entry.corrects = Some(LedgerTopicRef {
                    scope: "ops".to_owned(),
                    topic: "other-topic".to_owned(),
                });
                is_invalid(&build_ledger_record(entry, WRITER, None, NOW))
            },
        ),
        (
            "L6_correction_links_the_other_sessions_entry_under_its_own_key",
            other_entry.as_ref().is_some_and(|target| {
                let mut entry = input(
                    "ops",
                    "other-topic-correction",
                    LedgerKind::Correction,
                    "the other note was wrong",
                );
                entry.corrects = Some(LedgerTopicRef {
                    scope: "ops".to_owned(),
                    topic: "other-topic".to_owned(),
                });
                build_ledger_record(entry, WRITER, None, NOW).is_ok_and(|row| {
                    row.row_key != target.row_key
                        && row
                            .corrects
                            .as_ref()
                            .is_some_and(|link| link.topic == "other-topic")
                })
            }),
        ),
        ("L6_non_correction_with_a_corrects_link_is_refused", {
            let mut entry = input("ops", "t", LedgerKind::Useful, "text");
            entry.corrects = Some(LedgerTopicRef {
                scope: "ops".to_owned(),
                topic: "other-topic".to_owned(),
            });
            is_invalid(&build_ledger_record(entry, WRITER, None, NOW))
        }),
        (
            "L6_owner_notes_below_the_marker_are_kept_byte_for_byte",
            normal.as_ref().is_some_and(|row| {
                let owner_tail = format!("{OWNER_NOTES_MARKER}\nmy own note\r\n  - keep me\n");
                let existing = format!("# old daemon text\n### old / row\n\n{owner_tail}");
                let plan = render_ledger_file(std::slice::from_ref(row), Some(&existing));
                let text = written(&plan);
                text.ends_with(&owner_tail)
                    && !text.contains("old / row")
                    && marker_lines(text) == 1
            }),
        ),
        (
            "L6_owner_removed_the_marker_file_is_left_untouched",
            normal.as_ref().is_some_and(|row| {
                render_ledger_file(
                    std::slice::from_ref(row),
                    Some("owner rewrote everything\n"),
                ) == LedgerMirrorPlan::KeepOwnerFile
            }),
        ),
        (
            "L6_candidate_database_mirrors_to_its_own_file_never_the_live_one",
            {
                let live = ledger_mirror_path(Path::new(r"C:\data\synapse\db-daemon"));
                let candidate = ledger_mirror_path(Path::new(r"C:\data\synapse\db-candidate"));
                live == PathBuf::from(r"C:\data\synapse\db-daemon.ledger.md")
                    && candidate == PathBuf::from(r"C:\data\synapse\db-candidate.ledger.md")
            },
        ),
        // L7 restart
        (
            "L7_restart_row_key_holds_scope_and_topic_only_no_run_id",
            ledger_row_key("ops", "topic")
                == "workspace-ledger/v1/scope_hex/6f7073/topic_hex/746f706963",
        ),
        // L8 neighbour
        (
            "L8_neighbour_ledger_rows_are_outside_every_workspace_run_namespace",
            !ledger_row_key("ops", "topic").starts_with("workspace-blackboard/"),
        ),
        (
            "L8_neighbour_row_expires_at_its_expiry_instant_not_before",
            normal.as_ref().is_some_and(|row| {
                is_ledger_expired(row, row.expires_at_unix_ms)
                    && !is_ledger_expired(row, row.expires_at_unix_ms - 1)
            }),
        ),
        (
            "L8_neighbour_expired_current_row_is_replaced_as_new_version_1",
            normal.as_ref().is_some_and(|current| {
                build_ledger_record(dead_end(), OTHER, Some(current), current.expires_at_unix_ms)
                    .is_ok_and(|row| row.version == 1 && row.writer_session_id == OTHER)
            }),
        ),
        (
            "L8_neighbour_ledger_has_its_own_kv_budget_and_all_budgets_fit_the_cap",
            KV_NAMESPACE_QUOTAS
                .iter()
                .any(|quota| quota.key_prefix == "workspace-ledger/v1" && quota.quota_mb == 8)
                && KV_NAMESPACE_QUOTAS
                    .iter()
                    .map(|quota| quota.quota_mb)
                    .sum::<u64>()
                    < kv_hard_cap_mb(),
        ),
    ];

    let failures: Vec<String> = rows
        .iter()
        .filter(|(_, passed)| !passed)
        .map(|(row, _)| format!("row {row}: expected behaviour not observed"))
        .collect();
    assert!(
        failures.is_empty(),
        "ledger table rows failed:\n{}",
        failures.join("\n")
    );
}
