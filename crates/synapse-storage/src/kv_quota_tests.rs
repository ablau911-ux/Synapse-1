use std::collections::BTreeMap;

use crate::kv_quota::KvQuotaPolicy;

#[test]
fn must_pass_namespace_budgets_default_and_env_override() {
    let defaults = KvQuotaPolicy::from_lookup(|_| None);
    assert_eq!(defaults.quota_bytes("mailbox"), Some(16 * 1024 * 1024));
    assert_eq!(defaults.quota_bytes("workspace"), Some(16 * 1024 * 1024));

    let env = BTreeMap::from([
        ("SYNAPSE_KV_QUOTA_MAILBOX_MIB", " 4 "),
        ("SYNAPSE_KV_QUOTA_WORKSPACE_MIB", "0"),
    ]);
    let tuned = KvQuotaPolicy::from_lookup(|name| env.get(name).map(|raw| (*raw).to_owned()));
    assert_eq!(tuned.quota_bytes("mailbox"), Some(4 * 1024 * 1024));
    assert_eq!(
        tuned.quota_bytes("workspace"),
        Some(16 * 1024 * 1024),
        "zero or invalid overrides fall back to the default budget"
    );
}

#[test]
fn must_fail_critical_prefixes_are_exempt_only_when_every_row_is_critical() {
    let critical = vec![(b"approval/v1/item/a".to_vec(), vec![1])];
    let mixed = vec![
        (b"approval/v1/item/a".to_vec(), vec![1]),
        (b"agent-mailbox/v1/recipient_hex/x".to_vec(), vec![1]),
    ];
    assert!(KvQuotaPolicy::exempts_from_shared_cap("CF_KV", &critical));
    assert!(!KvQuotaPolicy::exempts_from_shared_cap("CF_KV", &mixed));
    assert!(!KvQuotaPolicy::exempts_from_shared_cap(
        "CF_SESSIONS",
        &critical
    ));
    assert!(!KvQuotaPolicy::exempts_from_shared_cap("CF_KV", &[]));
}
