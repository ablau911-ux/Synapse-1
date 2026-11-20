//! Size-capped segment rotation for the daemon tool-event ledger (#304).
//!
//! `LedgerRotation` holds the two caps in force (#2330) and does the whole
//! rotation: the pure decisions (`plan`, `steps`) are table-tested in
//! `daemon_ledger_rotation_table_304.rs` and `daemon_ledger_rotation_table_caps.rs`;
//! `rotate_if_full` applies them to the files.

use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::Path;

use anyhow::Context as _;

/// How the next record reaches the active ledger file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LedgerAppendPlan {
    /// Append to the active file as it is.
    Append,
    /// Retire the active file into segment `.1` first, then append to a fresh
    /// active file.
    RotateThenAppend,
}

/// One file step of a rotation; the steps are applied in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RotationStep {
    /// Delete retained segment `.N`, the oldest one.
    DeleteSegment(usize),
    /// Rename segment `.from` to `.to`.
    RenameSegment { from: usize, to: usize },
    /// Rename the active ledger to segment `.1`.
    RetireActive,
}

/// Default size cap of the active tool-event ledger before it rotates (8 MiB): one
/// segment stays quick to open and scan, rotation stays rare.
const MAX_LEDGER_SEGMENT_BYTES: u64 = 8 * 1024 * 1024;

/// Default number of rotated segments kept on disk (`.1` newest .. `.5` oldest), so the ledger
/// never holds more than about `MAX_LEDGER_SEGMENT_BYTES * 6` bytes.
const MAX_LEDGER_SEGMENTS: usize = 5;

/// Environment variable that sets the size cap of one ledger segment, in bytes.
const LEDGER_SEGMENT_BYTES_ENV: &str = "SYNAPSE_LEDGER_SEGMENT_BYTES";
/// Environment variable that sets how many rotated segments are kept.
const LEDGER_SEGMENTS_ENV: &str = "SYNAPSE_LEDGER_SEGMENTS";

/// Minimum configured size cap of one ledger segment, in bytes.
const MIN_CONFIGURED_SEGMENT_BYTES: u64 = 64 * 1024;
/// Maximum configured size cap of one ledger segment, in bytes.
const MAX_CONFIGURED_SEGMENT_BYTES: u64 = 1024 * 1024 * 1024;
/// Maximum configured number of rotated segments kept on disk.
const MAX_CONFIGURED_SEGMENTS: usize = 50;

/// The rotation of the tool-event ledger with the two caps in force.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LedgerRotation {
    segment_bytes: u64,
    segments: usize,
}

impl LedgerRotation {
    /// The caps in force for this process, read from the environment.
    #[must_use]
    pub(crate) fn from_env() -> Self {
        Self::from_settings(
            std::env::var(LEDGER_SEGMENT_BYTES_ENV).ok().as_deref(),
            std::env::var(LEDGER_SEGMENTS_ENV).ok().as_deref(),
        )
    }

    /// Parses the configured caps (#2330). Each value is read on its own: unset,
    /// unparsable or out of range keeps that value's default
    /// (`MAX_LEDGER_SEGMENT_BYTES`, `MAX_LEDGER_SEGMENTS`).
    pub(crate) fn from_settings(segment_bytes: Option<&str>, segments: Option<&str>) -> Self {
        Self {
            segment_bytes: segment_bytes
                .and_then(|value| value.trim().parse::<u64>().ok())
                .filter(|value| {
                    (MIN_CONFIGURED_SEGMENT_BYTES..=MAX_CONFIGURED_SEGMENT_BYTES).contains(value)
                })
                .unwrap_or(MAX_LEDGER_SEGMENT_BYTES),
            segments: segments
                .and_then(|value| value.trim().parse::<usize>().ok())
                .filter(|value| (1..=MAX_CONFIGURED_SEGMENTS).contains(value))
                .unwrap_or(MAX_LEDGER_SEGMENTS),
        }
    }

    /// Rotate only when the record would push a non-empty active file past the
    /// limit. A record that fits exactly is appended. An empty file never rotates,
    /// so a record larger than the limit is still written, alone in its segment.
    pub(crate) const fn plan(&self, active_bytes: u64, record_bytes: u64) -> LedgerAppendPlan {
        if active_bytes > 0 && active_bytes.saturating_add(record_bytes) > self.segment_bytes {
            LedgerAppendPlan::RotateThenAppend
        } else {
            LedgerAppendPlan::Append
        }
    }

    /// Shift scheme: drop the oldest segment, move every other segment up by one,
    /// then retire the active file into `.1`. Only `.1..=.segments` are named.
    pub(crate) fn steps(&self) -> Vec<RotationStep> {
        let max_segments = self.segments;
        let mut steps = Vec::with_capacity(max_segments.saturating_add(1));
        if max_segments > 0 {
            steps.push(RotationStep::DeleteSegment(max_segments));
        }
        for from in (1..max_segments).rev() {
            steps.push(RotationStep::RenameSegment { from, to: from + 1 });
        }
        steps.push(RotationStep::RetireActive);
        steps
    }

    /// `daemon-tool-events.jsonl` + index 3 -> `daemon-tool-events.jsonl.3`.
    pub(crate) fn segment_file_name(active_name: &str, index: usize) -> String {
        format!("{active_name}.{index}")
    }

    /// #304: before this record would push the active tool-event ledger past its
    /// size cap, retire it into numbered segments and drop the oldest one. Only
    /// this ledger's own numbered segment names are touched. A failure is returned, so the
    /// best-effort caller reports it in health and the record is not appended to
    /// an oversized file; the next record retries the rotation.
    pub(crate) fn rotate_if_full(&self, active: &Path, line: &[u8]) -> anyhow::Result<()> {
        let active_bytes = match fs::metadata(active) {
            Ok(metadata) => metadata.len(),
            Err(error) if error.kind() == io::ErrorKind::NotFound => 0,
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("stat daemon tool events {}", active.display()));
            }
        };
        let record_bytes = u64::try_from(line.len()).unwrap_or(u64::MAX);
        if self.plan(active_bytes, record_bytes) == LedgerAppendPlan::Append {
            return Ok(());
        }
        let active_name = active.file_name().and_then(OsStr::to_str).ok_or_else(|| {
            anyhow::anyhow!(
                "daemon tool events path has no file name: {}",
                active.display()
            )
        })?;
        let segment =
            |index: usize| active.with_file_name(Self::segment_file_name(active_name, index));
        for step in self.steps() {
            let result = match step {
                RotationStep::DeleteSegment(index) => fs::remove_file(segment(index)),
                RotationStep::RenameSegment { from, to } => fs::rename(segment(from), segment(to)),
                RotationStep::RetireActive => fs::rename(active, segment(1)),
            };
            match result {
                Ok(()) => {}
                // Fewer segments than the cap so far: a missing segment is normal.
                Err(error)
                    if error.kind() == io::ErrorKind::NotFound
                        && step != RotationStep::RetireActive => {}
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!("rotate daemon tool events {} at {step:?}", active.display())
                    });
                }
            }
        }
        tracing::info!(
            code = "MCP_DAEMON_LIFECYCLE_LEDGER_ROTATED",
            tool_events_path = %active.display(),
            active_bytes,
            record_bytes,
            max_segment_bytes = self.segment_bytes,
            max_segments = self.segments,
            "readback=daemon tool-event ledger rotated into numbered segments"
        );
        Ok(())
    }
}

#[cfg(test)]
#[path = "daemon_ledger_rotation_table_304.rs"]
mod table_304;

#[cfg(test)]
#[path = "daemon_ledger_rotation_table_caps.rs"]
mod table_caps;
