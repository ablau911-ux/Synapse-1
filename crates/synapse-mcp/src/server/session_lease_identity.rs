use super::{PersistedSessionLease, session_lease_key};
use synapse_action::LeaseStatus;
use synapse_storage::{Db, StorageResult, cf};

/// Keep the v1 row's source type compatible while persisting acquisition identity.
#[derive(serde::Serialize)]
struct LeaseIdentityRow<'a> {
    #[serde(flatten)]
    lease: &'a PersistedSessionLease,
    lease_generation: &'a Option<String>,
}

pub(super) fn encode_lease_row(
    row: &PersistedSessionLease,
    status: &LeaseStatus,
) -> StorageResult<Vec<u8>> {
    synapse_storage::encode_json(&LeaseIdentityRow {
        lease: row,
        lease_generation: &status.generation,
    })
}

pub(super) fn identity_matches(
    db: &Db,
    session_id: &str,
    status: &LeaseStatus,
    allow_legacy: bool,
) -> Result<bool, String> {
    let generation = read_generation(db, session_id)?;
    Ok(generation_matches(
        generation.as_deref(),
        status,
        allow_legacy,
    ))
}

pub(super) fn generation_matches(
    generation: Option<&str>,
    status: &LeaseStatus,
    allow_legacy: bool,
) -> bool {
    (allow_legacy && generation.is_none())
        || (status.generation.is_some() && generation == status.generation.as_deref())
}

/// Decode acquisition identity inside the scan's per-row corruption boundary.
pub(super) fn decode_lease_row(
    bytes: &[u8],
) -> Result<(PersistedSessionLease, Option<String>), String> {
    let row = synapse_storage::decode_json(bytes).map_err(|error| error.to_string())?;
    Ok((row, decode_generation(bytes)?))
}

pub(super) fn read_generation(db: &Db, session_id: &str) -> Result<Option<String>, String> {
    let key = session_lease_key(session_id);
    let rows = db
        .scan_cf_prefix(cf::CF_SESSIONS, &key)
        .map_err(|error| error.to_string())?;
    let Some((_, bytes)) = rows.into_iter().find(|(found, _)| found == &key) else {
        return Ok(None);
    };
    decode_generation(&bytes)
}

fn decode_generation(bytes: &[u8]) -> Result<Option<String>, String> {
    #[derive(serde::Deserialize)]
    struct Identity {
        lease_generation: Option<String>,
    }
    let identity: Identity = serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
    Ok(identity.lease_generation)
}
