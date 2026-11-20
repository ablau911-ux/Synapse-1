/// Default retention and size budget for one storage column family.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RetentionDefault {
    pub cf: &'static str,
    pub ttl: RetentionTtl,
    pub soft_cap_mb: u64,
    pub hard_cap_mb: u64,
}

/// Default TTL policy for a storage column family.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RetentionTtl {
    None,
    Hours(u64),
    Days(u64),
    LruOnly,
}

/// PRD §4/§6 storage retention defaults.
pub const DEFAULTS: [RetentionDefault; 17] = [
    RetentionDefault {
        cf: "CF_EVENTS",
        ttl: RetentionTtl::Hours(24),
        soft_cap_mb: 2048,
        hard_cap_mb: 4096,
    },
    RetentionDefault {
        cf: "CF_OBSERVATIONS",
        ttl: RetentionTtl::Hours(6),
        soft_cap_mb: 500,
        hard_cap_mb: 1000,
    },
    RetentionDefault {
        cf: "CF_PROFILES",
        ttl: RetentionTtl::None,
        soft_cap_mb: 20,
        hard_cap_mb: 50,
    },
    RetentionDefault {
        cf: "CF_MODEL_CACHE",
        ttl: RetentionTtl::LruOnly,
        soft_cap_mb: 1024,
        hard_cap_mb: 2048,
    },
    RetentionDefault {
        cf: "CF_SESSIONS",
        ttl: RetentionTtl::Days(30),
        soft_cap_mb: 50,
        hard_cap_mb: 100,
    },
    RetentionDefault {
        cf: "CF_REFLEX_AUDIT",
        ttl: RetentionTtl::Days(7),
        soft_cap_mb: 200,
        hard_cap_mb: 500,
    },
    RetentionDefault {
        cf: "CF_OCR_CACHE",
        ttl: RetentionTtl::Hours(1),
        soft_cap_mb: 50,
        hard_cap_mb: 100,
    },
    RetentionDefault {
        cf: "CF_TELEMETRY",
        ttl: RetentionTtl::Hours(6),
        soft_cap_mb: 100,
        hard_cap_mb: 200,
    },
    RetentionDefault {
        cf: "CF_ACTION_LOG",
        ttl: RetentionTtl::Hours(24),
        soft_cap_mb: 200,
        hard_cap_mb: 500,
    },
    RetentionDefault {
        cf: "CF_PROCESS_HISTORY",
        ttl: RetentionTtl::Hours(6),
        soft_cap_mb: 20,
        hard_cap_mb: 50,
    },
    RetentionDefault {
        cf: "CF_KV",
        ttl: RetentionTtl::None,
        soft_cap_mb: 10,
        hard_cap_mb: 50,
    },
    // ADR 2026-06-11-timeline-data-model: long-retention operator activity
    // timeline; TTL eviction additionally relies on periodic compaction so
    // cold SST files still pass the TTL filter (see storage cf_options).
    RetentionDefault {
        cf: "CF_TIMELINE",
        ttl: RetentionTtl::Days(90),
        soft_cap_mb: 4096,
        hard_cap_mb: 8192,
    },
    // Derived episodes (#846): same retention horizon as their source
    // timeline rows, far smaller footprint (one row per focused span, not
    // per event). Rebuildable at any time by re-segmentation.
    RetentionDefault {
        cf: "CF_EPISODES",
        ttl: RetentionTtl::Days(90),
        soft_cap_mb: 256,
        hard_cap_mb: 512,
    },
    // Derived routines (#848): a few hundred small rows replaced wholesale
    // on every mining run; the mining window (episode retention) bounds the
    // content, so no TTL — stale rows cannot outlive a re-mine.
    RetentionDefault {
        cf: "CF_ROUTINES",
        ttl: RetentionTtl::None,
        soft_cap_mb: 16,
        hard_cap_mb: 64,
    },
    // Operator routine lifecycle state (#849): confirmations, disables,
    // labels, transition audit trails. Operator decisions must never
    // silently expire, so no TTL; the store is bounded by the routine id
    // space (a few hundred rows) and per-row history caps.
    RetentionDefault {
        cf: "CF_ROUTINE_STATE",
        ttl: RetentionTtl::None,
        soft_cap_mb: 16,
        hard_cap_mb: 64,
    },
    // Durable agent-event journal (#897): the source of truth every Command
    // Center panel reconciles against (fleet metrics, transcripts, cost).
    // 30 days covers dashboard history without competing with CF_TIMELINE
    // for disk; TTL eviction additionally relies on periodic compaction so
    // cold SST files still pass the TTL filter (see storage cf_options).
    RetentionDefault {
        cf: "CF_AGENT_EVENTS",
        ttl: RetentionTtl::Days(30),
        soft_cap_mb: 512,
        hard_cap_mb: 1024,
    },
    // Normalized spawned-agent transcripts (#900): one row per source JSONL
    // line, re-ingestable from the spawn log files while those exist on
    // disk and the durable record after they are cleaned. Same 30-day
    // dashboard horizon as the agent-event journal it reconciles with.
    RetentionDefault {
        cf: "CF_AGENT_TRANSCRIPTS",
        ttl: RetentionTtl::Days(30),
        soft_cap_mb: 512,
        hard_cap_mb: 1024,
    },
];

/// Logical key + JSON value byte limit for permanent routine identity owners.
/// Refuse growth instead of evicting evidence that reserves historical ids.
pub const ROUTINE_IDENTITY_MAX_BYTES: usize = 1024 * 1024;

/// Per-namespace byte budget inside a shared column family (#164).
///
/// `CF_KV` has one shared hard cap; without namespace budgets one producer
/// (mailbox self-sends, 256 KiB workspace values) can fill it and starve every
/// other writer. A namespace is every key starting with `key_prefix`. The
/// budget can be overridden with `SYNAPSE_KV_QUOTA_<NAME>_MIB` (see
/// `synapse-storage`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NamespaceQuotaDefault {
    pub cf: &'static str,
    pub name: &'static str,
    pub key_prefix: &'static str,
    pub quota_mb: u64,
}

/// Bounded `CF_KV` namespaces. Their budgets sum to less than the `CF_KV`
/// hard cap, so the remainder stays reserved for everything else.
pub const KV_NAMESPACE_QUOTAS: [NamespaceQuotaDefault; 5] = [
    NamespaceQuotaDefault {
        cf: "CF_KV",
        name: "mailbox",
        key_prefix: "agent-mailbox/v1",
        quota_mb: 16,
    },
    NamespaceQuotaDefault {
        cf: "CF_KV",
        name: "workspace",
        key_prefix: "workspace-blackboard/v1",
        quota_mb: 16,
    },
    // Route memory (#2326): per-app tier stats, self-pruned per app.
    NamespaceQuotaDefault {
        cf: "CF_KV",
        name: "route_memory",
        key_prefix: "route-memory/v1",
        quota_mb: 4,
    },
    // Hand-off and lessons ledger (Phase 7 part 3): rows expire after 90
    // days and are removed on the next ledger write.
    NamespaceQuotaDefault {
        cf: "CF_KV",
        name: "ledger",
        key_prefix: "workspace-ledger/v1",
        quota_mb: 8,
    },
    // Action history (Phase 7 part 4): one row per finished mutating tool
    // call; rows past the retention days are removed on the next write.
    NamespaceQuotaDefault {
        cf: "CF_KV",
        name: "action_history",
        key_prefix: "action-history/v1",
        quota_mb: 4,
    },
];

/// `CF_KV` key prefixes whose writes are never refused by the shared hard cap
/// or a namespace budget: MCP sessions, approvals and escalations, shell-job
/// idempotency markers and agent tasks. These rows are small, bounded by live
/// state, and the daemon cannot make progress when they fail to write.
pub const KV_CRITICAL_PREFIXES: [&str; 6] = [
    "mcp/session/v1/",
    "mcp/session-lease/v1/",
    "approval/v1/",
    "escalation/v1/",
    "m4/act_run_shell/idempotency/v1/",
    "agent-task/v1/",
];
