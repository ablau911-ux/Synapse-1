//! #607 (memory step 4, "your routines"): the periodic routine miner reads
//! `CF_EPISODES`, but only `episode operation=segment` cut the timeline into
//! episodes, so the owner's real history never reached the miner unless
//! someone called that tool by hand. Before each periodic mining run the
//! episodes are brought up to date: the timeline is cut again from the day
//! of the newest stored episode (that day may have been cut while it was
//! still running) up to now; with no episode stored yet, from the first
//! timeline row. Human activity only, as the miner's default.

use rmcp::ErrorData;
use synapse_storage::{Db, cf, episodes as episode_codec};

use crate::m1::mcp_error;

use super::episodes::{
    EpisodeSegmentParams, EpisodeSegmentResponse, SCAN_CHUNK_ROWS, key_after, segment_episodes,
};

#[cfg(test)]
#[path = "episode_catch_up_table_607.rs"]
mod episode_catch_up_table_607;

/// One catch-up of `CF_EPISODES` with `CF_TIMELINE`: where the next
/// `episode_segment` call starts, and whether the episodes are up to date.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct EpisodeCatchUp {
    /// Start of the next segment call; `None` = the first timeline row.
    next_start_ts_ns: Option<u64>,
    /// True once a reply said the range is complete, or did not move forward.
    up_to_date: bool,
    /// Episodes written by the calls recorded so far (for the log line).
    episodes_written: u64,
}

impl EpisodeCatchUp {
    /// Starts at the newest stored episode (`None` when there is none).
    pub(super) const fn resume_from(newest_episode_start_ts_ns: Option<u64>) -> Self {
        Self {
            next_start_ts_ns: newest_episode_start_ts_ns,
            up_to_date: false,
            episodes_written: 0,
        }
    }

    /// The next segment call, or `None` once the episodes are up to date.
    pub(super) const fn next_call(&self) -> Option<EpisodeSegmentParams> {
        if self.up_to_date {
            return None;
        }
        Some(EpisodeSegmentParams {
            start_ts_ns: self.next_start_ts_ns,
            end_ts_ns: None,
            include_agent_activity: false,
            dry_run: false,
        })
    }

    /// Takes the reply of one segment call: continue where it paused, or stop.
    pub(super) fn record(&mut self, reply: &EpisodeSegmentResponse) {
        if self.up_to_date {
            return;
        }
        self.episodes_written = self.episodes_written.saturating_add(reply.episodes_written);
        match reply.next_start_ts_ns {
            // Paused further on: the next call starts there. `None` (the
            // first timeline row) is before every start.
            Some(next) if self.next_start_ts_ns.is_none_or(|current| next > current) => {
                self.next_start_ts_ns = Some(next);
            }
            // Complete, empty, or a pause that did not move forward (a loop
            // that never ends): the episodes are as up to date as they get.
            _ => self.up_to_date = true,
        }
    }

    /// Episodes written by the calls recorded so far.
    pub(super) const fn episodes_written(&self) -> u64 {
        self.episodes_written
    }

    /// Reads the newest stored episode, then runs segment calls until the
    /// episodes are up to date. Errors of a call end the catch-up.
    pub(super) fn run(db: &Db) -> Result<Self, ErrorData> {
        let mut catch_up = Self::resume_from(Self::newest_episode_start(db)?);
        while let Some(params) = catch_up.next_call() {
            let reply = segment_episodes(db, &params)?;
            catch_up.record(&reply);
        }
        Ok(catch_up)
    }

    /// Start of the newest stored episode (keys sort by start time), or
    /// `None` when `CF_EPISODES` is empty.
    fn newest_episode_start(db: &Db) -> Result<Option<u64>, ErrorData> {
        let mut start: Vec<u8> = Vec::new();
        let mut newest: Option<Vec<u8>> = None;
        loop {
            let (rows, more) = db
                .scan_cf_from(cf::CF_EPISODES, &start, SCAN_CHUNK_ROWS)
                .map_err(|error| mcp_error(error.code(), error.to_string()))?;
            let Some((last, _value)) = rows.last() else {
                break;
            };
            start = key_after(last);
            newest = Some(last.clone());
            if !more {
                break;
            }
        }
        newest
            .map(|key| {
                episode_codec::decode_episode_key(&key)
                    .map(|(start_ts_ns, _ordinal)| start_ts_ns)
                    .map_err(|error| mcp_error(error.code(), error.to_string()))
            })
            .transpose()
    }
}
