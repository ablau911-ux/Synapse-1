//! Content-derived stable routine identity (#165).
//!
//! A routine id must survive re-mining: operator lifecycle state
//! (`CF_ROUTINE_STATE`), armed automations, and automation rows all anchor on
//! it. Fresh candidates hash a step signature plus an independent time anchor.
//! Persisted mining restores permanent ownership before allocating any id.
//!
//! # Normalization
//!
//! Candidate hashing and legacy lookup normalize steps as follows:
//!
//! - app: whitespace trimmed/collapsed, Unicode-lowercased, one trailing
//!   `.exe` removed (`" Excel.EXE "` and `excel` are the same app);
//! - document: whitespace trimmed/collapsed, Unicode-lowercased, `\` folded to
//!   `/` (a missing document and an empty one are the same);
//! - granularity token and step order are significant.
//!
//! # What is deliberately excluded
//!
//! The day-of-week class, support, confidence, and the mining timestamp all
//! change as data accumulates; hashing them made ids shift between runs. The
//! time anchor disambiguates same-signature schedules independently of other
//! clusters, so inserting or removing an earlier schedule cannot renumber them.
//! Candidates sharing both signature and time anchor receive a deterministic
//! tie ordinal in raw candidate-key order; ordinals at other anchors cannot
//! affect their identity. Mining encodes the anchor and tie in the discriminator
//! passed to `routine_id`.
//!
//! # Persisted identity
//!
//! An anchor-derived id is not proof of identity across passes: two raw
//! spellings of one signature swap ids when their mean minutes cross, and a
//! sibling appearing or vanishing at a shared anchor shifts tie slots.
//! A raw identity means granularity plus the full ordered RoutineStep values
//! produced by the miner (after its existing ASCII lowercasing/collapse).
//! Suffixes, whitespace, document separators and None versus Some remain
//! significant here. Recorded owners match only these exact values, within
//! the maximum of the old/new circular schedule tolerances.
//!
//! Only a pre-ledger current record may bootstrap a cosmetic alias: each
//! normalized step field must match, the schedules must be near, and both
//! sides must have exactly one counterpart in that neighborhood. Any recorded
//! sibling makes that fallback ambiguous. The first ledger write freezes the
//! mined raw spelling; later signature equality cannot change its owner.
//!
//! Ownership survives both missing routines and removal of dependent rows.
//! Older v1 owners without schedules use a matching current record to recover
//! metadata, or a globally unique raw owner/candidate pair; otherwise their
//! ids stay reserved. Owner-unknown dependent keys also stay reserved.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::routines::{circular_distance, granularity_token};
use crate::types::{RoutineGranularity, RoutineRecord, RoutineStep};

#[path = "routine_identity_hash.rs"]
mod hashing;
use hashing::{displaced_routine_id, normalize_app, normalize_document};
pub use hashing::{routine_id, routine_signature};

/// Permanent raw owner of an id. Schedule metadata disambiguates repeated
/// occurrences of the same steps; it may move within the mining tolerance.
/// Old v1 rows without a schedule are recovered only when unambiguous.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoutineIdentityOwner {
    pub routine_id: String,
    pub granularity: RoutineGranularity,
    pub steps: Vec<RoutineStep>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schedule: Option<RoutineIdentitySchedule>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoutineIdentitySchedule {
    pub mean_minute_of_day: u32,
    pub tolerance_minutes: u32,
}

impl RoutineIdentityOwner {
    #[must_use]
    pub fn of(routine: &RoutineRecord) -> Self {
        Self {
            routine_id: routine.routine_id.clone(),
            granularity: routine.granularity,
            steps: routine.steps.clone(),
            schedule: Some(RoutineIdentitySchedule {
                mean_minute_of_day: routine.mean_minute_of_day,
                tolerance_minutes: routine.tolerance_minutes,
            }),
        }
    }

    #[must_use]
    pub fn owns_steps(&self, routine: &RoutineRecord) -> bool {
        self.granularity == routine.granularity && self.steps == routine.steps
    }

    fn signature(&self) -> String {
        routine_signature(self.granularity, &self.steps)
    }

    fn legacy_equivalent(&self, candidate: &RoutineRecord) -> bool {
        self.granularity == candidate.granularity
            && self.steps.len() == candidate.steps.len()
            && self.steps.iter().zip(&candidate.steps).all(|(a, b)| {
                normalize_app(&a.app) == normalize_app(&b.app)
                    && normalize_document(a.document.as_deref())
                        == normalize_document(b.document.as_deref())
            })
    }

    fn distance(&self, candidate: &RoutineRecord) -> u32 {
        self.schedule.map_or(u32::MAX, |schedule| {
            circular_distance(schedule.mean_minute_of_day, candidate.mean_minute_of_day)
        })
    }

    fn near(&self, candidate: &RoutineRecord) -> bool {
        self.schedule.is_some_and(|schedule| {
            self.distance(candidate) <= schedule.tolerance_minutes.max(candidate.tolerance_minutes)
        })
    }
}

/// Recorded owners are permanent. None reserves a pre-ledger id whose
/// dependent rows survive but whose raw identity cannot be recovered.
pub type RetainedIdentities = BTreeMap<String, Option<RoutineIdentityOwner>>;

/// Ledger evidence overrides current derived rows, never the reverse.
struct IdentityHistory {
    owners: BTreeMap<String, RoutineIdentityOwner>,
    recorded: BTreeSet<String>,
    reserved: BTreeSet<String>,
}

impl IdentityHistory {
    fn new(old: &[RoutineRecord], retained: &RetainedIdentities) -> Self {
        let mut owners: BTreeMap<_, _> = old
            .iter()
            .map(|routine| {
                (
                    routine.routine_id.clone(),
                    RoutineIdentityOwner::of(routine),
                )
            })
            .collect();
        let mut recorded = BTreeSet::new();
        for (id, owner) in retained {
            if let Some(owner) = owner {
                let owner = recover_schedule(owner, old);
                owners.insert(id.clone(), owner);
                recorded.insert(id.clone());
            }
        }
        let reserved = owners.keys().chain(retained.keys()).cloned().collect();
        Self {
            owners,
            recorded,
            reserved,
        }
    }

    fn ordered(&self, recorded: bool) -> Vec<&RoutineIdentityOwner> {
        let mut owners: Vec<_> = self
            .owners
            .values()
            .filter(|owner| self.recorded.contains(&owner.routine_id) == recorded)
            .collect();
        owners.sort_by_key(|owner| {
            (
                owner.schedule.map(|schedule| schedule.mean_minute_of_day),
                owner.routine_id.as_str(),
            )
        });
        owners
    }

    fn unique_raw(&self, owner: &RoutineIdentityOwner, new: &[RoutineRecord]) -> bool {
        self.owners
            .values()
            .filter(|other| other.granularity == owner.granularity && other.steps == owner.steps)
            .count()
            == 1
            && new.iter().filter(|row| owner.owns_steps(row)).count() == 1
    }

    fn legacy_pair_is_unique(
        &self,
        owner: &RoutineIdentityOwner,
        new: &[RoutineRecord],
        index: usize,
    ) -> bool {
        let signature = owner.signature();
        self.owners
            .values()
            .filter(|other| {
                other.signature() == signature
                    && (other.near(&new[index]) || other.schedule.is_none())
            })
            .count()
            == 1
            && new
                .iter()
                .filter(|row| {
                    routine_signature(row.granularity, &row.steps) == signature && owner.near(row)
                })
                .count()
                == 1
    }
}

fn recover_schedule(owner: &RoutineIdentityOwner, old: &[RoutineRecord]) -> RoutineIdentityOwner {
    let mut recovered = owner.clone();
    if owner.schedule.is_none() {
        recovered.schedule = old
            .iter()
            .find(|row| row.routine_id == owner.routine_id && owner.owns_steps(row))
            .and_then(|row| RoutineIdentityOwner::of(row).schedule);
    }
    recovered
}

/// Allocation never reclaims an existing id; ownership matching does that
/// first, with raw identity and schedule evidence.
struct IdClaims {
    reserved: BTreeSet<String>,
    used: BTreeSet<String>,
    fresh: BTreeSet<String>,
}

impl IdClaims {
    fn may_take(&self, id: &str) -> bool {
        !self.used.contains(id) && !self.reserved.contains(id)
    }

    fn may_take_derived(&self, id: &str) -> bool {
        self.may_take(id) && !self.fresh.contains(id)
    }
}

/// Signature fallback is only a one-time bootstrap for unrecorded,
/// mutually unambiguous legacy schedules.
#[derive(Clone, Copy)]
enum MatchPass {
    Exact,
    RawSteps,
    Unscheduled,
    Legacy,
}

impl MatchPass {
    fn accepts(self, owner: &RoutineIdentityOwner, candidate: &RoutineRecord) -> bool {
        let same = owner.owns_steps(candidate);
        match self {
            Self::Exact => {
                same && owner.near(candidate) && candidate.routine_id == owner.routine_id
            }
            Self::RawSteps => same && owner.near(candidate),
            Self::Unscheduled => same && owner.schedule.is_none(),
            Self::Legacy => owner.near(candidate) && owner.legacy_equivalent(candidate),
        }
    }
}

struct CandidatePool<'a> {
    records: &'a [RoutineRecord],
    assigned: Vec<Option<String>>,
    used: BTreeSet<String>,
}

impl<'a> CandidatePool<'a> {
    fn new(records: &'a [RoutineRecord]) -> Self {
        Self {
            records,
            assigned: vec![None; records.len()],
            used: BTreeSet::new(),
        }
    }

    fn claim(&mut self, pass: MatchPass, owner: &RoutineIdentityOwner, history: &IdentityHistory) {
        if self.used.contains(&owner.routine_id) {
            return;
        }
        let best = self
            .records
            .iter()
            .enumerate()
            .filter(|(index, row)| {
                self.assigned[*index].is_none()
                    && pass.accepts(owner, row)
                    && match pass {
                        MatchPass::Unscheduled => history.unique_raw(owner, self.records),
                        MatchPass::Legacy => {
                            history.legacy_pair_is_unique(owner, self.records, *index)
                        }
                        _ => true,
                    }
            })
            .min_by(|(_, a), (_, b)| {
                (owner.distance(a), &a.routine_id)
                    .cmp(&(owner.distance(b), &b.routine_id))
                    .then_with(|| compare_raw_steps(a, b))
            })
            .map(|(index, _)| index);
        if let Some(index) = best {
            self.assigned[index] = Some(owner.routine_id.clone());
            self.used.insert(owner.routine_id.clone());
        }
    }
}

fn compare_raw_steps(a: &RoutineRecord, b: &RoutineRecord) -> std::cmp::Ordering {
    granularity_token(a.granularity)
        .cmp(granularity_token(b.granularity))
        .then_with(|| {
            a.steps
                .iter()
                .map(|step| (&step.app, &step.document))
                .cmp(b.steps.iter().map(|step| (&step.app, &step.document)))
        })
}

fn assign_owners(history: &IdentityHistory, new: &[RoutineRecord]) -> Vec<Option<String>> {
    let mut pool = CandidatePool::new(new);
    // Recorded ownership wins even over an exact id in an unrecorded snapshot.
    for recorded in [true, false] {
        let owners = history.ordered(recorded);
        for pass in [
            MatchPass::Exact,
            MatchPass::RawSteps,
            MatchPass::Unscheduled,
        ] {
            for owner in &owners {
                pool.claim(pass, owner, history);
            }
        }
    }
    for owner in history.ordered(false) {
        pool.claim(MatchPass::Legacy, owner, history);
    }
    pool.assigned
}

/// Reconciles current records with no additional retained ledger evidence.
pub fn reconcile_routine_identities(old: &[RoutineRecord], new: &mut [RoutineRecord]) {
    reconcile_routine_identities_retained(old, &RetainedIdentities::new(), new);
}

/// Restores retained owners before considering legacy aliases or allocating
/// fresh ids. All historical ids, including owner-unknown rows, stay reserved.
pub fn reconcile_routine_identities_retained(
    old: &[RoutineRecord],
    retained: &RetainedIdentities,
    new: &mut [RoutineRecord],
) {
    let history = IdentityHistory::new(old, retained);
    let mut assigned = assign_owners(&history, new);
    let mut claims = IdClaims {
        reserved: history.reserved,
        used: assigned.iter().flatten().cloned().collect(),
        fresh: new.iter().map(|row| row.routine_id.clone()).collect(),
    };
    let mut order: Vec<_> = (0..new.len()).collect();
    order.sort_by(|a, b| {
        new[*a]
            .routine_id
            .cmp(&new[*b].routine_id)
            .then_with(|| compare_raw_steps(&new[*a], &new[*b]))
    });
    for index in order {
        if assigned[index].is_some() {
            continue;
        }
        let row = &new[index];
        let id = if claims.may_take(&row.routine_id) {
            row.routine_id.clone()
        } else {
            displaced_routine_id(row, &claims)
        };
        claims.used.insert(id.clone());
        assigned[index] = Some(id);
    }
    for (row, id) in new.iter_mut().zip(assigned) {
        if let Some(id) = id {
            row.routine_id = id;
        }
    }
}

/// Legacy matching without ledger evidence. Returns (old id, candidate id)
/// only when ids differ; raw matching still precedes cosmetic fallback.
#[must_use]
pub fn match_routines_by_signature(
    old: &[RoutineRecord],
    new: &[RoutineRecord],
) -> Vec<(String, String)> {
    match_routines_by_signature_retained(old, &RetainedIdentities::new(), new)
}

/// Migration uses the same ownership decisions as recurring mining. A target
/// id already present in the ledger cannot receive another owner's rows.
#[must_use]
pub fn match_routines_by_signature_retained(
    old: &[RoutineRecord],
    retained: &RetainedIdentities,
    new: &[RoutineRecord],
) -> Vec<(String, String)> {
    let history = IdentityHistory::new(old, retained);
    let old_ids: BTreeSet<_> = old.iter().map(|row| row.routine_id.as_str()).collect();
    assign_owners(&history, new)
        .into_iter()
        .zip(new)
        .filter_map(|(owner, row)| {
            let owner = owner?;
            (old_ids.contains(owner.as_str())
                && !history.recorded.contains(&owner)
                && owner != row.routine_id
                && !retained.contains_key(&row.routine_id))
            .then(|| (owner, row.routine_id.clone()))
        })
        .collect()
}
