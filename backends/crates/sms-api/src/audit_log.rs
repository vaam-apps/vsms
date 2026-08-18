#![doc = include_str!("audit_log.md")]

use chrono::{DateTime, Utc};
// `cratestack::sqlx` the module, not individual items — see `worker_locks.rs`'s
// identical comment: `cargo xtask no-raw-sqlx`'s pattern matches the literal
// substring `sqlx::query`, so the raw call stays visible at the call site.
use cratestack::CratestackError;
use cratestack::sqlx;
use sha2::{Digest, Sha256};

use crate::schema::{AuditAnchor, Cratestack, audit_anchor};

/// Fixed 32-byte value folded in as the "previous row" when there is
/// nothing earlier to fold — see [`fold_rows`].
const ROW_FOLD_GENESIS: [u8; 32] = [0u8; 32];

/// `AuditAnchor.prevChainHash` on the very first anchor this deployment
/// ever writes — see `schema.cstack`'s own field doc for why this is a
/// fixed sentinel rather than `NULL`.
#[must_use]
pub fn genesis_hex() -> String {
    "0".repeat(64)
}

/// One `cratestack_audit` row, decoded. Field names mirror the DDL exactly
/// (`cratestack-sqlx-0.7.10/src/audit.rs` + `src/audit/schema.rs`, read
/// directly): `event_id UUID PRIMARY KEY, schema_name TEXT, model TEXT,
/// operation TEXT, primary_key JSONB, actor JSONB, tenant TEXT, before
/// JSONB, after JSONB, request_id TEXT, occurred_at TIMESTAMPTZ NOT NULL`.
#[derive(Debug, Clone)]
pub struct AuditRow {
    pub event_id: cratestack::uuid::Uuid,
    pub schema_name: String,
    pub model: String,
    pub operation: String,
    pub primary_key: serde_json::Value,
    pub actor: serde_json::Value,
    pub tenant: Option<String>,
    pub before: Option<serde_json::Value>,
    pub after: Option<serde_json::Value>,
    pub request_id: Option<String>,
    pub occurred_at: DateTime<Utc>,
}

/// `true` if `error` is Postgres's `42P01 undefined_table` — a fresh
/// deployment that has never written an audit row has, by construction, no
/// table to query yet.
fn is_undefined_table(error: &sqlx::Error) -> bool {
    error
        .as_database_error()
        .and_then(sqlx::error::DatabaseError::code)
        .as_deref()
        == Some("42P01")
}

#[allow(clippy::type_complexity)]
type FetchedRow = (
    cratestack::uuid::Uuid,
    String,
    String,
    String,
    serde_json::Value,
    serde_json::Value,
    Option<String>,
    Option<serde_json::Value>,
    Option<serde_json::Value>,
    Option<String>,
    DateTime<Utc>,
);

fn decode_row(row: FetchedRow) -> AuditRow {
    let (
        event_id,
        schema_name,
        model,
        operation,
        primary_key,
        actor,
        tenant,
        before,
        after,
        request_id,
        occurred_at,
    ) = row;
    AuditRow {
        event_id,
        schema_name,
        model,
        operation,
        primary_key,
        actor,
        tenant,
        before,
        after,
        request_id,
        occurred_at,
    }
}

/// Every `cratestack_audit` row with `occurred_at` in `(period_start,
/// period_end]` — `period_start = None` means no lower bound. Ordered
/// `(occurred_at, event_id)` so folding is deterministic across repeated
/// reads of the same, unmodified rows. Used only by [`fold_rows`]'s own
/// callers ([`verify_period_content`], and `anchor_audit`'s own write
/// path) — [`list_audit_entries`] below has its own, differently-ordered,
/// differently-filtered query, because a human browsing the log wants
/// newest-first with pagination, not a hash-fold's oldest-first window.
pub async fn rows_in_period(
    db: &Cratestack,
    period_start: Option<DateTime<Utc>>,
    period_end: DateTime<Utc>,
) -> Result<Vec<AuditRow>, sqlx::Error> {
    let rows: Vec<FetchedRow> = match sqlx::query_as(
        "SELECT event_id, schema_name, model, operation, primary_key, actor, tenant, before, \
         after, request_id, occurred_at \
         FROM cratestack_audit \
         WHERE ($1::timestamptz IS NULL OR occurred_at > $1) AND occurred_at <= $2 \
         ORDER BY occurred_at ASC, event_id ASC",
    )
    .bind(period_start)
    .bind(period_end)
    .fetch_all(db.pool())
    .await
    {
        Ok(rows) => rows,
        Err(error) if is_undefined_table(&error) => Vec::new(),
        Err(error) => return Err(error),
    };

    Ok(rows.into_iter().map(decode_row).collect())
}

/// Writes `bytes.len()` as an 8-byte big-endian prefix, then `bytes`
/// itself — the length-prefixed framing every variable-length field below
/// needs, so two adjacent variable-length fields can't shift boundaries and
/// hash identically for two genuinely different rows.
fn write_len_prefixed(hasher: &mut Sha256, bytes: &[u8]) {
    hasher.update((bytes.len() as u64).to_be_bytes());
    hasher.update(bytes);
}

/// A one-byte presence flag, then [`write_len_prefixed`] if present.
fn write_optional_str(hasher: &mut Sha256, value: Option<&str>) {
    match value {
        Some(text) => {
            hasher.update([1u8]);
            write_len_prefixed(hasher, text.as_bytes());
        }
        None => hasher.update([0u8]),
    }
}

/// A JSON value serialized with every object's keys sorted, recursively —
/// deterministic regardless of `serde_json`'s own `Map` ordering or
/// Postgres's `jsonb` internal key order on the way back out through
/// `sqlx`.
fn canonical_json(value: &serde_json::Value) -> String {
    fn sorted(value: &serde_json::Value) -> serde_json::Value {
        match value {
            serde_json::Value::Object(map) => {
                let mut keys: Vec<&String> = map.keys().collect();
                keys.sort();
                let mut ordered = serde_json::Map::new();
                for key in keys {
                    ordered.insert(key.clone(), sorted(&map[key]));
                }
                serde_json::Value::Object(ordered)
            }
            serde_json::Value::Array(items) => {
                serde_json::Value::Array(items.iter().map(sorted).collect())
            }
            other => other.clone(),
        }
    }
    serde_json::to_string(&sorted(value)).unwrap_or_default()
}

/// One row's own digest — every column that can vary between two
/// otherwise-identical-looking rows, in a fixed, framed order. Changing any
/// field of any row changes this.
fn hash_audit_row(row: &AuditRow) -> [u8; 32] {
    let mut hasher = Sha256::new();
    write_len_prefixed(&mut hasher, row.event_id.as_bytes());
    write_len_prefixed(&mut hasher, row.schema_name.as_bytes());
    write_len_prefixed(&mut hasher, row.model.as_bytes());
    write_len_prefixed(&mut hasher, row.operation.as_bytes());
    write_len_prefixed(&mut hasher, canonical_json(&row.primary_key).as_bytes());
    write_len_prefixed(&mut hasher, canonical_json(&row.actor).as_bytes());
    write_optional_str(&mut hasher, row.tenant.as_deref());
    let before_json = row.before.as_ref().map(canonical_json);
    write_optional_str(&mut hasher, before_json.as_deref());
    let after_json = row.after.as_ref().map(canonical_json);
    write_optional_str(&mut hasher, after_json.as_deref());
    write_optional_str(&mut hasher, row.request_id.as_deref());
    hasher.update(row.occurred_at.timestamp_micros().to_be_bytes());
    hasher.finalize().into()
}

/// Sequential fold over every row in order:
/// `acc = SHA256(acc || hash_audit_row(row))`, starting from
/// [`ROW_FOLD_GENESIS`]. `pub` for `anchor_audit.rs`'s own write path.
#[must_use]
pub fn fold_rows(rows: &[AuditRow]) -> [u8; 32] {
    let mut acc = ROW_FOLD_GENESIS;
    for row in rows {
        let row_hash = hash_audit_row(row);
        let mut hasher = Sha256::new();
        hasher.update(acc);
        hasher.update(row_hash);
        acc = hasher.finalize().into();
    }
    acc
}

/// `AuditAnchor.chainHash` — commits to `prevChainHash` *and* this anchor's
/// own metadata (`periodStart`/`periodEnd`/`rowCount`), not just
/// `rangeHash`, so tampering with an anchor row's own fields is exactly as
/// detectable as tampering with the audit rows it covers.
#[must_use]
pub fn compute_chain_hash_hex(
    prev_chain_hash_hex: &str,
    period_start: Option<DateTime<Utc>>,
    period_end: DateTime<Utc>,
    row_count: i64,
    range_hash_hex: &str,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(prev_chain_hash_hex.as_bytes());
    match period_start {
        Some(start) => {
            hasher.update([1u8]);
            hasher.update(start.timestamp_micros().to_be_bytes());
        }
        None => hasher.update([0u8]),
    }
    hasher.update(period_end.timestamp_micros().to_be_bytes());
    hasher.update(row_count.to_be_bytes());
    hasher.update(range_hash_hex.as_bytes());
    hex::encode(hasher.finalize())
}

/// The most recent `AuditAnchor` by `periodEnd`, or `None` on a fresh
/// deployment that has never anchored anything. `pub` for `anchor_audit.rs`'s
/// own write path and this module's own [`chain_status`].
///
/// # Errors
///
/// Whatever the underlying `find_many` returns.
pub async fn latest_anchor(
    db: &Cratestack,
    sys: &cratestack::CratestackContext,
) -> Result<Option<AuditAnchor>, CratestackError> {
    let mut rows = db
        .audit_anchor()
        .find_many()
        .order_by(audit_anchor::periodEnd().desc())
        .limit(1)
        .run(sys)
        .await?;
    Ok(rows.pop())
}

/// Walks every `AuditAnchor`, oldest first, and checks two things per
/// anchor: its `prevChainHash` equals the actual previous anchor's
/// `chainHash` (or the genesis sentinel, for the oldest anchor on record),
/// and its own `chainHash` still equals what recomputing it from that
/// anchor's own stored fields produces. Returns a human-readable
/// description per break found, rather than an error — a linkage break is
/// exactly the finding this exists to surface, not a fault that should stop
/// `anchor_audit` from also anchoring today's rows, or stop the console's
/// own [`chain_status`] from reporting what it found.
///
/// # Errors
///
/// Whatever the underlying `find_many` returns.
pub async fn verify_chain_linkage(
    db: &Cratestack,
    sys: &cratestack::CratestackContext,
) -> Result<Vec<String>, CratestackError> {
    let anchors = db
        .audit_anchor()
        .find_many()
        .order_by(audit_anchor::periodEnd().asc())
        .run(sys)
        .await?;

    let mut breaks = Vec::new();
    let mut previous_chain_hash: Option<String> = None;

    for anchor in &anchors {
        let expected_prev = previous_chain_hash.clone().unwrap_or_else(genesis_hex);
        if anchor.prevChainHash != expected_prev {
            breaks.push(format!(
                "anchor {} prevChainHash ({}) does not equal the expected previous link ({})",
                anchor.id, anchor.prevChainHash, expected_prev
            ));
        }

        let recomputed = compute_chain_hash_hex(
            &anchor.prevChainHash,
            anchor.periodStart,
            anchor.periodEnd,
            anchor.rowCount,
            &anchor.rangeHash,
        );
        if recomputed != anchor.chainHash {
            breaks.push(format!(
                "anchor {} chainHash ({}) does not match its own stored fields (recomputed {})",
                anchor.id, anchor.chainHash, recomputed
            ));
        }

        previous_chain_hash = Some(anchor.chainHash.clone());
    }

    Ok(breaks)
}

/// Re-reads `cratestack_audit` for exactly `anchor`'s own `(periodStart,
/// periodEnd]` and checks whether folding those rows still produces
/// `anchor.rangeHash`.
///
/// # Errors
///
/// A raw `sqlx::Error` from the underlying query — the same failure mode
/// [`rows_in_period`] can produce.
pub async fn verify_period_content(
    db: &Cratestack,
    anchor: &AuditAnchor,
) -> Result<bool, sqlx::Error> {
    let rows = rows_in_period(db, anchor.periodStart, anchor.periodEnd).await?;
    let recomputed = hex::encode(fold_rows(&rows));
    Ok(recomputed == anchor.rangeHash)
}

/// A snapshot of the audit chain's own health — the "does this period's
/// chain verify" signal #58's own issue asked for, computed live rather
/// than cached, so it always reflects the current state of the table
/// (cheap: at most one anchor row per scheduled run, and one period's worth
/// of `cratestack_audit` rows to re-fold — see `anchor_audit.rs`'s own
/// module doc on why a full-history re-verification is deliberately not
/// this function's job either).
#[derive(Debug, Clone)]
pub struct ChainStatus {
    /// `None` on a deployment that has never anchored anything — the
    /// `anchor_audit` job hasn't run yet, not a chain failure.
    pub latest_anchor: Option<AuditAnchor>,
    /// Every linkage break [`verify_chain_linkage`] found, across the whole
    /// chain — empty means every anchor's own `prevChainHash`/`chainHash`
    /// is internally consistent.
    pub linkage_breaks: Vec<String>,
    /// Whether the most recent anchor's own stored `rangeHash` still
    /// matches a fresh fold of the `cratestack_audit` rows it claims to
    /// cover. `None` when there is no anchor to check, or when the
    /// recomputation itself failed (a raw SQL error, logged by the caller
    /// rather than surfaced here — a transient failure to *check* is not
    /// the same claim as "the chain is broken").
    pub latest_period_content_verified: Option<bool>,
}

/// Builds a [`ChainStatus`] — the whole-chain linkage check plus a fresh
/// content re-verification of the most recent period. Read-only: this
/// function is called from the `auditChainStatus` procedure and from
/// nowhere else that could confuse "checking" with "anchoring" — it never
/// writes an `AuditAnchor` row (only `anchor_audit`'s own scheduled job
/// does that).
///
/// # Errors
///
/// [`CratestackError`] from the underlying `AuditAnchor` reads. A failure to
/// re-verify the latest period's own *content* (a raw SQL error) is not
/// propagated as an error here — see [`ChainStatus::latest_period_content_verified`]'s
/// own doc for why that's a `None`, not a hard failure of the whole
/// procedure.
pub async fn chain_status(
    db: &Cratestack,
    sys: &cratestack::CratestackContext,
) -> Result<ChainStatus, CratestackError> {
    let latest = latest_anchor(db, sys).await?;
    let linkage_breaks = verify_chain_linkage(db, sys).await?;

    let latest_period_content_verified = match &latest {
        Some(anchor) => verify_period_content(db, anchor).await.ok(),
        None => None,
    };

    Ok(ChainStatus {
        latest_anchor: latest,
        linkage_breaks,
        latest_period_content_verified,
    })
}

/// One row of the console's own audit-log view — a lossless, presentational
/// flattening of [`AuditRow`], JSON fields kept as their canonical string
/// form rather than parsed further (the same convention `Provider.config`/
/// `Route.config` already use for JSON-shaped `String` columns) rather than
/// this crate inventing a second JSON scalar type just for this screen.
#[derive(Debug, Clone)]
pub struct AuditLogEntry {
    pub event_id: String,
    pub model: String,
    pub operation: String,
    pub primary_key: String,
    pub actor: String,
    pub tenant: Option<String>,
    pub before: Option<String>,
    pub after: Option<String>,
    pub request_id: Option<String>,
    pub occurred_at: DateTime<Utc>,
}

fn to_entry(row: AuditRow) -> AuditLogEntry {
    AuditLogEntry {
        event_id: row.event_id.to_string(),
        model: row.model,
        operation: row.operation,
        primary_key: canonical_json(&row.primary_key),
        actor: canonical_json(&row.actor),
        tenant: row.tenant,
        before: row.before.as_ref().map(canonical_json),
        after: row.after.as_ref().map(canonical_json),
        request_id: row.request_id,
        occurred_at: row.occurred_at,
    }
}

/// What #58's audit-log screen may filter by. Every field is an exact
/// match, not a substring search — `model`/`operation` are closed-ish
/// vocabularies (model names, `"create"`/`"update"`/`"delete"`) where a
/// typo'd filter should return zero rows, not a confusing partial match.
/// `actor_id` matches `actor->>'id'`, the one sub-field of the `actor` JSONB
/// blob every audited write populates (`cratestack-sqlx`'s own
/// `build_audit_event` — read directly, not assumed) — "who did this" is
/// the question this filter answers.
#[derive(Debug, Clone, Default)]
pub struct AuditLogFilter {
    pub model: Option<String>,
    pub operation: Option<String>,
    pub actor_id: Option<String>,
    pub since: Option<DateTime<Utc>>,
    pub until: Option<DateTime<Utc>>,
}

/// Server-side cap on a single page — the same "never trust a caller-supplied
/// limit unboundedly" discipline `messages.ts`'s own `MAX_SERVER_LIMIT`
/// documents on the TypeScript side, mirrored here since this query isn't
/// behind the generated router's own `@@paged` machinery (there is no model,
/// hence no `@@paged`, to enforce it for us).
pub const MAX_AUDIT_LOG_LIMIT: i64 = 200;

/// One page of the audit log, newest first — [`AuditLogEntry`]s plus
/// whether a further page exists, fetched by requesting one row more than
/// `limit` and trimming it off (the same "over-fetch by one" trick as every
/// other cursor-free pager in this codebase).
#[derive(Debug, Clone)]
pub struct AuditLogPage {
    pub entries: Vec<AuditLogEntry>,
    pub has_more: bool,
}

/// `SELECT ... FROM cratestack_audit WHERE ... ORDER BY occurred_at DESC,
/// event_id DESC LIMIT $n OFFSET $o` — a human paging through recent
/// history newest-first, the opposite order and opposite audience from
/// [`rows_in_period`]'s own oldest-first hash-fold window.
/// Deliberately a second query rather than one shared function with an
/// order-direction flag: the two callers have nothing else in common (one
/// folds a fixed period into a hash with no pagination or filtering at all,
/// the other pages and filters with no folding), and a shared function
/// parameterised over both would be harder to read than two short ones.
///
/// # Errors
///
/// A raw `sqlx::Error`, or `CratestackError::Validation` if `limit`/`offset` are
/// negative.
pub async fn list_audit_entries(
    db: &Cratestack,
    filter: &AuditLogFilter,
    limit: i64,
    offset: i64,
) -> Result<AuditLogPage, CratestackError> {
    if limit < 0 || offset < 0 {
        return Err(CratestackError::Validation(
            "limit and offset must not be negative".to_owned(),
        ));
    }
    let capped_limit = limit.clamp(1, MAX_AUDIT_LOG_LIMIT);

    let rows: Vec<FetchedRow> = match sqlx::query_as(
        "SELECT event_id, schema_name, model, operation, primary_key, actor, tenant, before, \
         after, request_id, occurred_at \
         FROM cratestack_audit \
         WHERE ($1::text IS NULL OR model = $1) \
           AND ($2::text IS NULL OR operation = $2) \
           AND ($3::text IS NULL OR actor->>'id' = $3) \
           AND ($4::timestamptz IS NULL OR occurred_at >= $4) \
           AND ($5::timestamptz IS NULL OR occurred_at <= $5) \
         ORDER BY occurred_at DESC, event_id DESC \
         LIMIT $6 OFFSET $7",
    )
    .bind(filter.model.as_deref())
    .bind(filter.operation.as_deref())
    .bind(filter.actor_id.as_deref())
    .bind(filter.since)
    .bind(filter.until)
    .bind(capped_limit + 1)
    .bind(offset)
    .fetch_all(db.pool())
    .await
    {
        Ok(rows) => rows,
        Err(error) if is_undefined_table(&error) => Vec::new(),
        Err(error) => {
            return Err(CratestackError::Internal(format!(
                "reading cratestack_audit for the console audit log: {error}"
            )));
        }
    };

    let has_more = rows.len() > usize::try_from(capped_limit).unwrap_or(usize::MAX);
    let entries = rows
        .into_iter()
        .take(usize::try_from(capped_limit).unwrap_or(usize::MAX))
        .map(decode_row)
        .map(to_entry)
        .collect();

    Ok(AuditLogPage { entries, has_more })
}

#[cfg(test)]
mod tests {
    use super::{
        AuditRow, ROW_FOLD_GENESIS, canonical_json, compute_chain_hash_hex, fold_rows, genesis_hex,
        hash_audit_row,
    };
    use chrono::{TimeZone, Utc};

    fn sample_row(model: &str, occurred_at: chrono::DateTime<Utc>) -> AuditRow {
        AuditRow {
            event_id: cratestack::uuid::Uuid::from_u128(1),
            schema_name: String::new(),
            model: model.to_owned(),
            operation: "create".to_owned(),
            primary_key: serde_json::json!({"id": "abc123"}),
            actor: serde_json::json!({"id": "user1", "claims": null, "ip": null}),
            tenant: None,
            before: None,
            after: Some(serde_json::json!({"id": "abc123", "name": "test"})),
            request_id: Some("req-1".to_owned()),
            occurred_at,
        }
    }

    #[test]
    fn genesis_hex_is_exactly_64_characters() {
        let genesis = genesis_hex();
        assert_eq!(genesis.len(), 64);
        assert!(genesis.chars().all(|c| c == '0'));
    }

    #[test]
    fn canonical_json_ignores_key_order() {
        let a = serde_json::json!({"z": 1, "a": 2, "m": {"y": 1, "b": 2}});
        let b = serde_json::json!({"a": 2, "m": {"b": 2, "y": 1}, "z": 1});
        assert_eq!(canonical_json(&a), canonical_json(&b));
    }

    #[test]
    fn canonical_json_distinguishes_different_content() {
        let a = serde_json::json!({"a": 1});
        let b = serde_json::json!({"a": 2});
        assert_ne!(canonical_json(&a), canonical_json(&b));
    }

    #[test]
    fn hashing_the_same_row_twice_is_deterministic() {
        let now = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        assert_eq!(
            hash_audit_row(&sample_row("App", now)),
            hash_audit_row(&sample_row("App", now))
        );
    }

    #[test]
    fn changing_any_field_changes_the_row_hash() {
        let now = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let base = sample_row("App", now);
        let base_hash = hash_audit_row(&base);

        let mut different_model = sample_row("App", now);
        different_model.model = "Provider".to_owned();
        assert_ne!(hash_audit_row(&different_model), base_hash);

        let mut different_after = sample_row("App", now);
        different_after.after = Some(serde_json::json!({"id": "abc123", "name": "tampered"}));
        assert_ne!(hash_audit_row(&different_after), base_hash);
    }

    #[test]
    fn fold_rows_of_zero_rows_is_the_fixed_genesis_fold() {
        assert_eq!(fold_rows(&[]), ROW_FOLD_GENESIS);
    }

    #[test]
    fn fold_rows_is_order_sensitive() {
        let now = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let mut first = sample_row("App", now);
        first.event_id = cratestack::uuid::Uuid::from_u128(1);
        let mut second = sample_row("Provider", now);
        second.event_id = cratestack::uuid::Uuid::from_u128(2);

        let forward = vec![clone_row(&first), clone_row(&second)];
        let reversed = vec![clone_row(&second), clone_row(&first)];
        assert_ne!(fold_rows(&forward), fold_rows(&reversed));
    }

    fn clone_row(row: &AuditRow) -> AuditRow {
        AuditRow {
            event_id: row.event_id,
            schema_name: row.schema_name.clone(),
            model: row.model.clone(),
            operation: row.operation.clone(),
            primary_key: row.primary_key.clone(),
            actor: row.actor.clone(),
            tenant: row.tenant.clone(),
            before: row.before.clone(),
            after: row.after.clone(),
            request_id: row.request_id.clone(),
            occurred_at: row.occurred_at,
        }
    }

    #[test]
    fn compute_chain_hash_hex_is_64_hex_characters() {
        let hash = compute_chain_hash_hex(&genesis_hex(), None, Utc::now(), 0, &genesis_hex());
        assert_eq!(hash.len(), 64);
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn compute_chain_hash_hex_changes_with_any_input() {
        let now = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let base = compute_chain_hash_hex(&genesis_hex(), None, now, 3, &genesis_hex());
        assert_ne!(
            compute_chain_hash_hex(&"1".repeat(64), None, now, 3, &genesis_hex()),
            base
        );
        assert_ne!(
            compute_chain_hash_hex(&genesis_hex(), Some(now), now, 3, &genesis_hex()),
            base
        );
        assert_ne!(
            compute_chain_hash_hex(&genesis_hex(), None, now, 4, &genesis_hex()),
            base
        );
        assert_ne!(
            compute_chain_hash_hex(&genesis_hex(), None, now, 3, &"1".repeat(64)),
            base
        );
    }
}
