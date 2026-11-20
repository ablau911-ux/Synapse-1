//! #607 second part: the periodic routine miner mines a rolling window of
//! recent days instead of the whole history. Over 85 days of real history
//! every pattern that recurs on enough days also runs at every other hour, so
//! the whole-history arc is rejected as dispersed and nothing is proposed;
//! a window of recent weeks keeps time-anchored routines visible and bounds
//! the scan. `SYNAPSE_ROUTINE_MINE_WINDOW_DAYS` sets the window (default 28
//! days; `0` = the whole history); an invalid value stops the daemon at
//! startup like the other miner settings.

#[cfg(test)]
#[path = "mining_window_table_607.rs"]
mod mining_window_table_607;

/// Environment variable: days of history the periodic miner mines.
pub const WINDOW_DAYS_ENV: &str = "SYNAPSE_ROUTINE_MINE_WINDOW_DAYS";
/// Default window: 28 days (four weeks).
pub const DEFAULT_WINDOW_DAYS: u64 = 28;
/// Nanoseconds in one day.
const DAY_NS: u64 = 86_400_000_000_000;

/// The rolling window of the periodic miner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct MiningWindow {
    /// Days of history to mine; 0 = the whole history.
    days: u64,
}

impl MiningWindow {
    /// From the raw environment value (`None` = not set).
    ///
    /// # Errors
    ///
    /// A value that is not an unsigned whole number of days.
    pub(super) fn from_env_value(raw: Option<&str>) -> Result<Self, String> {
        let trimmed = raw.unwrap_or("").trim();
        if trimmed.is_empty() {
            return Ok(Self {
                days: DEFAULT_WINDOW_DAYS,
            });
        }
        trimmed
            .parse::<u64>()
            .map(|days| Self { days })
            .map_err(|error| {
                format!(
                    "{WINDOW_DAYS_ENV} must be an unsigned whole number of days (0 = the whole history); got {trimmed:?}: {error}"
                )
            })
    }

    /// Days of history mined (0 = the whole history).
    pub(super) const fn days(&self) -> u64 {
        self.days
    }

    /// First timestamp mined for a run at `now_ts_ns`; `None` = the whole history.
    pub(super) const fn start_ts_ns(&self, now_ts_ns: u64) -> Option<u64> {
        if self.days == 0 {
            return None;
        }
        Some(now_ts_ns.saturating_sub(self.days.saturating_mul(DAY_NS)))
    }
}
