//! Best-effort, bounded expiry index; the primary mailbox row is authoritative.
use super::*;

#[cfg(test)]
#[path = "tests/mailbox_cleanup_blocking_57.rs"]
pub(super) mod test_cleanup_57;

const EXPIRY_PREFIX: &str = "agent-mailbox-expiry/v1/";
const EXPIRY_PAGE_ROWS: usize = 128;
type IndexStates = std::collections::HashMap<usize, (std::sync::Weak<Db>, IndexState)>;
static INDEX_STATES: std::sync::Mutex<Option<IndexStates>> = std::sync::Mutex::new(None);

#[derive(Default)]
struct IndexState {
    cursor: Option<Vec<u8>>,
    expiry_cursor: Option<Vec<u8>>,
    ready: bool,
    corrupt_skipped: usize,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExpiryEntry {
    schema_version: u32,
    row_key: String,
    to_session: String,
    message_id: String,
    expires_at_unix_ms: u64,
}

pub(super) fn expiry_key(message: &AgentMailboxMessage) -> Vec<u8> {
    format!(
        "{EXPIRY_PREFIX}{:020}/{}/{}",
        message.expires_at_unix_ms,
        hex_bytes(message.to_session.as_bytes()),
        message.message_id
    )
    .into_bytes()
}

pub(super) fn expiry_row(message: &AgentMailboxMessage) -> Result<(Vec<u8>, Vec<u8>), ErrorData> {
    let value = ExpiryEntry {
        schema_version: SCHEMA_VERSION,
        row_key: message.row_key.clone(),
        to_session: message.to_session.clone(),
        message_id: message.message_id.clone(),
        expires_at_unix_ms: message.expires_at_unix_ms,
    };
    let encoded = synapse_storage::encode_json(&value)
        .map_err(|error| mcp_error(error.code(), error.to_string()))?;
    Ok((expiry_key(message), encoded))
}

fn source(db: &Db, key: &[u8]) -> Result<Option<AgentMailboxMessage>, ErrorData> {
    let Some(value) = db
        .get_cf_exact(cf::CF_KV, key)
        .map_err(|error| mcp_error(error.code(), error.to_string()))?
    else {
        return Ok(None);
    };
    let message: AgentMailboxMessage = match synapse_storage::decode_json(&value) {
        Ok(message) => message,
        Err(error) => {
            note_corrupt_mailbox_row(key, &error.to_string());
            return Ok(None);
        }
    };
    let canonical = std::str::from_utf8(key)
        .ok()
        .and_then(|key| key.strip_prefix(&mailbox_recipient_prefix(&message.to_session)))
        .and_then(|suffix| suffix.split('/').nth(1))
        .and_then(|seq| seq.parse::<u64>().ok())
        .map(|seq| {
            mailbox_row_key(
                &message.to_session,
                message.sent_at_unix_ms,
                seq,
                &message.message_id,
            )
        });
    if message.schema_version != SCHEMA_VERSION
        || message.row_key.as_bytes() != key
        || canonical
            .as_ref()
            .is_none_or(|canonical| canonical.as_bytes() != key)
        || message.expires_at_unix_ms != message.sent_at_unix_ms.saturating_add(message.ttl_ms)
    {
        note_corrupt_mailbox_row(key, "expiry source identity mismatch");
        return Ok(None);
    }
    Ok(Some(message))
}

fn index_legacy_page(db: &Db, state: &mut IndexState) -> Result<(), ErrorData> {
    if state.ready {
        return Ok(());
    }
    let cursor = state.cursor.as_deref().unwrap_or(MESSAGE_PREFIX.as_bytes());
    let (rows, more) = db
        .scan_cf_from(cf::CF_KV, cursor, EXPIRY_PAGE_ROWS)
        .map_err(|error| mcp_error(error.code(), error.to_string()))?;
    // An empty initial namespace has no legacy rows to reconcile yet.
    if state.cursor.is_none()
        && !rows
            .iter()
            .any(|(key, _)| key.starts_with(MESSAGE_PREFIX.as_bytes()))
    {
        return Ok(());
    }
    let (puts, next_cursor, ready, corrupt_skipped) = legacy_entries(db, rows, state)?;
    db.put_batch_pressure_bypass(cf::CF_KV, puts)
        .map_err(|error| mcp_error(error.code(), error.to_string()))?;
    state.cursor = next_cursor;
    state.corrupt_skipped += corrupt_skipped;
    state.ready = ready || !more;
    Ok(())
}

type LegacyEntries = (Vec<(Vec<u8>, Vec<u8>)>, Option<Vec<u8>>, bool, usize);

fn legacy_entries(
    db: &Db,
    rows: Vec<(Vec<u8>, Vec<u8>)>,
    state: &IndexState,
) -> Result<LegacyEntries, ErrorData> {
    let mut puts = Vec::new();
    let mut next_cursor = state.cursor.clone();
    let mut ready = false;
    let mut corrupt_skipped = 0;
    for (key, _) in rows {
        if !key.starts_with(MESSAGE_PREFIX.as_bytes()) {
            ready = true;
            break;
        }
        let mut next = key.clone();
        next.push(0);
        next_cursor = Some(next);
        if let Some(message) = source(db, &key)? {
            puts.push(expiry_row(&message)?);
        } else {
            corrupt_skipped += 1;
        }
    }
    Ok((puts, next_cursor, ready, corrupt_skipped))
}

fn sweep_entry(db: &Db, key: Vec<u8>, value: &[u8], now: u64) -> Result<usize, ErrorData> {
    let entry: ExpiryEntry = match synapse_storage::decode_json(value) {
        Ok(entry) => entry,
        Err(error) => {
            note_corrupt_mailbox_row(&key, &error.to_string());
            return Ok(0);
        }
    };
    let expected = format!(
        "{EXPIRY_PREFIX}{:020}/{}/{}",
        entry.expires_at_unix_ms,
        hex_bytes(entry.to_session.as_bytes()),
        entry.message_id
    );
    if entry.schema_version != SCHEMA_VERSION
        || expected.as_bytes() != key
        || !entry
            .row_key
            .starts_with(&mailbox_recipient_prefix(&entry.to_session))
    {
        note_corrupt_mailbox_row(&key, "expiry index identity mismatch");
        return Ok(0);
    }
    let Some(message) = source(db, entry.row_key.as_bytes())? else {
        // A missing or corrupt source cannot authorize a primary deletion.
        db.delete_batch(cf::CF_KV, [key])
            .map_err(|error| mcp_error(error.code(), error.to_string()))?;
        return Ok(0);
    };
    if expiry_key(&message) != key {
        db.mutate_batch_pressure_bypass(cf::CF_KV, [key], [expiry_row(&message)?])
            .map_err(|error| mcp_error(error.code(), error.to_string()))?;
        return Ok(0);
    }
    if message.expires_at_unix_ms > now {
        return Ok(0);
    }
    db.delete_batch(cf::CF_KV, [entry.row_key.into_bytes(), key])
        .map_err(|error| mcp_error(error.code(), error.to_string()))?;
    Ok(1)
}

pub(super) fn cleanup(
    db: &Arc<Db>,
    clock: &impl Fn() -> u64,
) -> Result<Option<MailboxSweep>, ErrorData> {
    // Housekeeping never waits behind a send/drain. Failures remain best-effort
    // at the existing throttle boundary; initialization never refuses a send.
    let _mutation = match MAILBOX_DRAINS.try_lock() {
        Ok(guard) => guard,
        Err(std::sync::TryLockError::WouldBlock) => return Ok(None),
        Err(std::sync::TryLockError::Poisoned(error)) => error.into_inner(),
    };
    let mut states = INDEX_STATES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let states = states.get_or_insert_with(std::collections::HashMap::new);
    states.retain(|_, (owner, _)| owner.strong_count() > 0);
    let (_, state) = states
        .entry(Arc::as_ptr(db) as usize)
        .or_insert_with(|| (Arc::downgrade(db), IndexState::default()));
    index_legacy_page(db, state)?;
    sweep_due_page(db, state, clock).map(Some)
}

fn expiry_page(db: &Db, state: &IndexState) -> Result<synapse_storage::ScanWindow, ErrorData> {
    let cursor = state
        .expiry_cursor
        .as_deref()
        .unwrap_or(EXPIRY_PREFIX.as_bytes());
    db.scan_cf_from(cf::CF_KV, cursor, EXPIRY_PAGE_ROWS)
        .map_err(|error| mcp_error(error.code(), error.to_string()))
}

fn sweep_due_page(
    db: &Db,
    state: &mut IndexState,
    clock: &impl Fn() -> u64,
) -> Result<MailboxSweep, ErrorData> {
    let mut result = MailboxSweep {
        deleted: 0,
        corrupt_skipped: state.corrupt_skipped,
    };
    let (rows, more) = expiry_page(db, state)?;
    for (key, value) in rows {
        if !key.starts_with(EXPIRY_PREFIX.as_bytes()) {
            state.expiry_cursor = None;
            break;
        }
        let mut next = key.clone();
        next.push(0);
        let due = key
            .get(EXPIRY_PREFIX.len()..EXPIRY_PREFIX.len() + 20)
            .and_then(|raw| std::str::from_utf8(raw).ok())
            .and_then(|raw| raw.parse::<u64>().ok());
        let Some(due) = due else {
            note_corrupt_mailbox_row(&key, "invalid expiry timestamp");
            result.corrupt_skipped += 1;
            state.expiry_cursor = Some(next);
            continue;
        };
        if due > clock() {
            state.expiry_cursor = None;
            break;
        }
        result.deleted += sweep_entry(db, key, &value, clock())?;
        state.expiry_cursor = Some(next);
    }
    if !more {
        state.expiry_cursor = None;
    }
    Ok(result)
}
