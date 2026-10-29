#![doc = include_str!("procedures.md")]

use chrono::{DateTime, Datelike, Duration as ChronoDuration, Timelike, Utc};
use cratestack::{CratestackContext, CratestackError, Decimal, FilterExpr, Value};
use sms_core::pack;
use sms_encoding::{SmsEncoding, analyse, normalise, transliterate_to_gsm7};
use sms_msisdn::{Msisdn, OperatorPrefixTable};
use tracing::info;

use crate::audit_log;
use crate::auth::PrincipalKind;
use crate::cache::TtlCache;
use crate::errors::map_database_error;
use crate::hmac_gate::HmacWebhooks;
use crate::pepper::{HashPepper, hmac_sha256_hex};
use crate::rbac::require_permission;
use crate::route_simulator;
use crate::schema::{
    self, app, app_client, client_key, consent_record, delivery_receipt, job, message,
    operator_prefix_rule, opt_out, provider, sender_id, sender_id_registration, webhook_attempt,
    webhook_endpoint,
};
use crate::worker_locks;

/// `OperatorPrefixRule`'s delegate, taken by the two helpers that both a
/// plain procedure (`simulateRoute`) and an `@isolation` one (`sendMessage`)
/// call — the isolated handle and `Cratestack` are different types, but
/// each hands out this same delegate.
type OperatorPrefixRules<'a> =
    cratestack::ModelDelegate<'a, schema::models::OperatorPrefixRule, String>;

/// Marker for a procedure whose backing subsystem is not built yet.
fn not_yet(procedure: &str, milestone: &str) -> CratestackError {
    CratestackError::Internal(format!(
        "{procedure} is not implemented: it depends on work scheduled for {milestone}"
    ))
}

/// Map this crate's encoding verdict onto the schema's `Encoding` enum.
fn encoding_of(encoding: SmsEncoding) -> schema::Encoding {
    match encoding {
        SmsEncoding::Gsm7 => schema::Encoding::gsm7,
        SmsEncoding::Ucs2 => schema::Encoding::ucs2,
    }
}

/// Distinct offending characters, first-occurrence order.
///
/// [`analyse`](sms_encoding::analyse) reports every occurrence so a composer can
/// highlight each one; the wire type is a flat `String[]`, where twenty copies
/// of `ç` is noise rather than information.
fn distinct_offending(report: &sms_encoding::EncodingReport) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    for offending in &report.offending {
        let ch = offending.ch.to_string();
        if !seen.contains(&ch) {
            seen.push(ch);
        }
    }
    seen
}

/// `OperatorCode`'s wire form, matching how `OperatorPrefixRule.operator` is
/// stored — the same lowercase-verbatim convention every enum in this
/// schema uses. A plain `match`, not a `Display` derive, so the mapping is
/// exactly as wide as the enum and a new variant is a compile error here
/// rather than a silent gap.
pub(crate) const fn operator_code_str(code: schema::OperatorCode) -> &'static str {
    match code {
        schema::OperatorCode::mtn => "mtn",
        schema::OperatorCode::orange => "orange",
        schema::OperatorCode::camtel => "camtel",
        schema::OperatorCode::nexttel => "nexttel",
        schema::OperatorCode::unknown => "unknown",
    }
}

/// The inverse of [`operator_code_str`]. `None` on anything else — an
/// `OperatorPrefixRule` row with a value this crate doesn't recognise
/// should not silently become `unknown`'s *meaning* ("no rule matched");
/// it is a data problem worth surfacing, not swallowing.
pub(crate) fn parse_operator_code(s: &str) -> Option<schema::OperatorCode> {
    Some(match s {
        "mtn" => schema::OperatorCode::mtn,
        "orange" => schema::OperatorCode::orange,
        "camtel" => schema::OperatorCode::camtel,
        "nexttel" => schema::OperatorCode::nexttel,
        "unknown" => schema::OperatorCode::unknown,
        _ => return None,
    })
}

/// The first instant of `now`'s UTC calendar month — the quota window's
/// start. UTC because nothing in the schema carries a timezone; a
/// documented, simple boundary beats a locally-correct one this crate has
/// no data to compute.
fn month_start(now: DateTime<Utc>) -> DateTime<Utc> {
    now.with_day(1)
        .and_then(|d| d.with_hour(0))
        .and_then(|d| d.with_minute(0))
        .and_then(|d| d.with_second(0))
        .and_then(|d| d.with_nanosecond(0))
        .unwrap_or(now)
}

/// §7.4: *"Default validity 15 minutes for `otp`... 24 hours for
/// `notification`."* Extended here to every non-`otp` class, since the doc
/// names `notification` as the example, not as the only non-OTP case, and a
/// `transactional` or `marketing` message defaulting to `otp`'s aggressive
/// 15-minute window would expire messages nobody intended to be that
/// time-sensitive.
fn default_validity(class: schema::MessageClass) -> ChronoDuration {
    match class {
        schema::MessageClass::otp => ChronoDuration::minutes(15),
        schema::MessageClass::transactional
        | schema::MessageClass::notification
        | schema::MessageClass::marketing => ChronoDuration::hours(24),
    }
}

/// Implementations behind the generated router.
///
/// `Clone`, per `ProcedureRegistry`'s own bound — the framework holds one
/// logical `Procedures` and clones it across request handling. The caches
/// are `Arc`-wrapped so every clone shares the same underlying state rather
/// than each starting cold; a `Procedures` clone is a couple of pointer
/// copies, not a cache reset.
#[derive(Clone)]
pub struct Procedures {
    /// `clientId → App`, §3.2: *"That's on the hot path, so cache it — 60
    /// seconds is short enough that retiring a client takes effect
    /// promptly and long enough that the lookup never matters."*
    app_cache: std::sync::Arc<TtlCache<String, schema::App>>,
    /// The operator-prefix table, singleton (unit key). Five minutes: no
    /// value is stated anywhere for this one, unlike the app cache's 60s —
    /// chosen longer because prefix assignments change on the order of
    /// months, not seconds, and this is queried on *every* send and
    /// preview, not just once per client.
    operator_cache: std::sync::Arc<TtlCache<(), OperatorPrefixTable>>,
    /// #134: the server-held pepper behind `msisdnHash`/`bodyHash` — see
    /// `pepper.rs`'s module doc for the scheme, and for the rotation
    /// consequence of ever changing it. `Clone` on `HashPepper` is a cheap
    /// `Arc` bump, matching the two caches above.
    pepper: HashPepper,
    /// #49: `dashboardSummary`'s own snapshot, keyed by the caller's
    /// `kind:role:appId` (see `dashboard_cache_key`) — a human/system
    /// caller and an app-scoped caller must never share an entry, since
    /// they see different rows. 15s: short enough that the dashboard still
    /// reads as live against the console's own poll interval, long enough
    /// that two browser tabs open on the same dashboard don't double the
    /// ~26-query cost `dashboard_snapshot`'s own doc explains.
    dashboard_cache: std::sync::Arc<TtlCache<String, schema::DashboardSummary>>,
    /// VSA-1: whether `auth.methods` permits `hmac_webhooks`. `rotateWebhookSecret`
    /// and a `switchWebhookDeliveryMode` to `hmac` refuse (422,
    /// `auth_method_disabled`) when it does not. [`Self::new`] permits it, which is
    /// what an absent `auth.methods` means; the router always sets it explicitly
    /// ([`Self::with_hmac_webhooks`]).
    hmac_webhooks: HmacWebhooks,
}

// No `Default` impl on purpose (#134): a default would have to invent a
// pepper, and this repo's own standing preference is a hard cutover, never
// a silently-weaker fallback — see AGENTS.md's "Delivery style" section.
// Every construction site must supply a real one explicitly.

impl Procedures {
    /// `pepper` is real secret material — see `pepper.rs`'s module doc.
    /// There is no default: every caller (the router, every live-Postgres
    /// test, `examples/send_test_message.rs`) must supply one explicitly.
    #[must_use]
    pub fn new(pepper: HashPepper) -> Self {
        Self {
            app_cache: std::sync::Arc::new(TtlCache::new(std::time::Duration::from_mins(1))),
            operator_cache: std::sync::Arc::new(TtlCache::new(std::time::Duration::from_mins(5))),
            pepper,
            dashboard_cache: std::sync::Arc::new(TtlCache::new(std::time::Duration::from_secs(15))),
            hmac_webhooks: HmacWebhooks::Permitted,
        }
    }

    /// Whether `auth.methods` permits `hmac_webhooks` (VSA-1). The default is
    /// [`HmacWebhooks::Permitted`], an absent key's meaning; see [`HmacWebhooks`].
    #[must_use]
    pub fn with_hmac_webhooks(mut self, hmac_webhooks: HmacWebhooks) -> Self {
        self.hmac_webhooks = hmac_webhooks;
        self
    }

    /// `{HASH_SCHEME}:{hex}` — see `pepper.rs`'s `hmac_sha256_hex` for the
    /// algorithm. The one place `sendMessage` turns a plaintext MSISDN or
    /// body into the value persisted in `msisdnHash`/`bodyHash`.
    fn keyed_hash_hex(&self, input: &str) -> String {
        hmac_sha256_hex(&self.pepper, input)
    }

    /// A `system`-role context — the only one the send path's app
    /// resolution and its own reads and writes admit. Built
    /// fresh per call rather than cached: it carries no state worth
    /// reusing, and constructing it is a handful of `Value::String`
    /// allocations, not a query. See [`crate::auth::system_context`] for
    /// the shared constructor and the invariant it documents.
    fn sys() -> CratestackContext {
        crate::auth::system_context("sms-api:procedures")
    }

    /// Analyse a body, and normalise a recipient if one was supplied.
    ///
    /// Runs [`normalise`] before [`analyse`], because normalisation is
    /// unconditional on the send path — previewing the raw body would quote a
    /// segment count the caller will never actually be billed for.
    fn preview(args: &schema::PreviewInput) -> Result<schema::PreviewResult, CratestackError> {
        let normalised = normalise(&args.body);
        let report = analyse(&normalised);

        let normalized_to = args
            .to
            .as_deref()
            .filter(|to| !to.trim().is_empty())
            .map(|to| {
                Msisdn::parse_mobile(to)
                    .map(|m| m.as_e164().to_owned())
                    .map_err(|error| CratestackError::Validation(error.to_string()))
            })
            .transpose()?;

        Ok(schema::PreviewResult {
            encoding: encoding_of(report.encoding),
            // `segments` is a u8, so this widening is infallible.
            segments: i64::from(report.segments),
            length: i64::try_from(report.length).unwrap_or(i64::MAX),
            perSegment: i64::try_from(report.per_segment).unwrap_or(i64::MAX),
            offending: distinct_offending(&report),
            suggestion: report.suggestion.clone(),
            // Real classification exists now (see `classify_operator`,
            // used by `sendMessage`) but isn't wired in here: this
            // function is intentionally synchronous and DB-free (it's
            // milestone 0's stated gate), and OperatorPrefixRule's own
            // policy denies read to anything but owner/frontends/apps/admin/operator/
            // auditor, so classifying for real would mean this procedure
            // reading under a system context the way sendMessage does —
            // a real change, not a follow-up left for later by accident.
            operator: schema::OperatorCode::unknown,
            normalizedTo: normalized_to,
        })
    }

    /// `client_id → AppClient → App`, cached. §3.2's hot-path lookup.
    ///
    /// Only ever called with a machine (`kind == "app"`) caller's own
    /// `sub` — see [`Procedures::caller_client_id`], which is what rules
    /// out a human caller before this is reached.
    async fn resolve_app(
        &self,
        db: &schema::IsolatedCratestack,
        sys: &CratestackContext,
        client_id: String,
    ) -> Result<schema::App, CratestackError> {
        self.app_cache
            .get_or_fetch(client_id, |client_id| async move {
                let app_client = db
                    .app_client()
                    .find_many()
                    .where_expr(
                        FilterExpr::from(app_client::clientId().eq(client_id.as_str()))
                            .and(app_client::active().is_true()),
                    )
                    .limit(1)
                    .run(sys)
                    .await?
                    .into_iter()
                    .next()
                    .ok_or_else(|| CratestackError::Unauthorized("unknown client".to_owned()))?;

                db.app()
                    .find_many()
                    .where_expr(
                        FilterExpr::from(app::id().eq(app_client.appId))
                            .and(app::active().is_true()),
                    )
                    .limit(1)
                    .run(sys)
                    .await?
                    .into_iter()
                    .next()
                    .ok_or_else(|| {
                        CratestackError::Unauthorized("app not found or inactive".to_owned())
                    })
            })
            .await
    }

    /// The cached prefix table, rebuilt from `OperatorPrefixRule` on a
    /// cache miss.
    async fn operator_table(
        &self,
        rules: OperatorPrefixRules<'_>,
        sys: &CratestackContext,
    ) -> Result<OperatorPrefixTable, CratestackError> {
        self.operator_cache
            .get_or_fetch((), |()| async move {
                let rows = rules
                    .find_many()
                    .where_expr(FilterExpr::from(operator_prefix_rule::active().is_true()))
                    .run(sys)
                    .await?;
                Ok(OperatorPrefixTable::new(rows.into_iter().map(|row| {
                    (row.prefix, operator_code_str(row.operator).to_owned())
                })))
            })
            .await
    }

    /// Infer the operator for `msisdn`. `unknown` on no match — never a
    /// guess, per `sms-msisdn::operator`'s own doc: a routing *hint*, never
    /// load-bearing.
    async fn classify_operator(
        &self,
        rules: OperatorPrefixRules<'_>,
        sys: &CratestackContext,
        msisdn: &Msisdn,
    ) -> Result<schema::OperatorCode, CratestackError> {
        let table = self.operator_table(rules, sys).await?;
        Ok(table
            .lookup(msisdn)
            .and_then(parse_operator_code)
            .unwrap_or(schema::OperatorCode::unknown))
    }

    /// §3.2 step: opt-out check, on the hashed MSISDN, before anything else
    /// that would justify persisting a row.
    ///
    /// #72: only called for classes where
    /// `consent::requires_recipient_consent_controls` is true — see that
    /// function's own doc for why (§10: "for `marketing` and
    /// `notification`. OTP and transactional are exempt"). Before #72 this
    /// ran unconditionally for every class, which silently blocked OTP/
    /// transactional delivery to a recipient who had only ever opted out
    /// of marketing — more restrictive than the design doc specified, not
    /// a compliance gap, but not what was asked for either.
    async fn ensure_not_opted_out(
        db: &schema::IsolatedCratestack,
        sys: &CratestackContext,
        msisdn_hash: &str,
    ) -> Result<(), CratestackError> {
        let opted_out = db
            .opt_out()
            .find_many()
            .where_expr(FilterExpr::from(opt_out::msisdnHash().eq(msisdn_hash)))
            .limit(1)
            .run(sys)
            .await?;
        if opted_out.is_empty() {
            Ok(())
        } else {
            Err(CratestackError::Validation(
                "recipient has opted out of messages from this scope".to_owned(),
            ))
        }
    }

    /// #72: does this message's *delivery* time fall inside the
    /// self-imposed marketing quiet-hours window? Only ever called when
    /// `consent::subject_to_quiet_hours(class)` is true — see
    /// `consent.rs`'s own module doc for the enforcement-location and
    /// policy-vs-statute reasoning.
    ///
    /// **Takes the delivery time, not the accept time**, and the
    /// distinction is the whole point. The first cut of this check passed
    /// `now`, which a caller could bypass trivially: accept a `marketing`
    /// message at 10:00 WAT — comfortably inside the window — with
    /// `scheduledAt` set to 22:00, and `claim.rs`'s own `candidates()`
    /// (`scheduledAt IS NULL OR scheduledAt <= now`) holds the row until
    /// 22:00 and then dispatches it, squarely inside the quiet hours this
    /// function exists to enforce. Caught in review of #213, and confirmed
    /// against all three links — the check's argument, the persisted
    /// `scheduledAt`, and the claim filter — rather than argued from the
    /// signature alone.
    ///
    /// So the argument is `scheduledAt.unwrap_or(now)`: what the recipient
    /// experiences, which is the only thing the policy is about.
    fn ensure_within_marketing_quiet_hours(
        delivery_utc: DateTime<Utc>,
    ) -> Result<(), CratestackError> {
        if crate::consent::is_within_marketing_quiet_hours(delivery_utc) {
            Ok(())
        } else {
            Err(CratestackError::Validation(format!(
                "marketing messages are only sent between {:02}:00 and {:02}:00 WAT \
                 (self-imposed quiet hours, docs/architecture.md §10) — schedule this send \
                 for that window instead",
                crate::consent::MARKETING_QUIET_HOURS_START_WAT,
                crate::consent::MARKETING_QUIET_HOURS_END_WAT,
            )))
        }
    }

    /// #72: is there a standing `ConsentRecord` on file for this
    /// `(appId, msisdnHash, class)` triple? Only ever called when
    /// `consent::requires_recipient_consent_controls(class)` is true.
    ///
    /// Matches on `msisdnHash`, not plaintext `msisdn` — the same
    /// pepper-rotation and post-purge caveat `ensure_not_opted_out`
    /// already lives with (see `ConsentRecord`'s own `schema.cstack`
    /// comment and `sms_api::pepper`'s module doc).
    async fn ensure_consent_on_file(
        db: &schema::IsolatedCratestack,
        sys: &CratestackContext,
        app_id: &str,
        msisdn_hash: &str,
        class: schema::MessageClass,
    ) -> Result<(), CratestackError> {
        let consented = db
            .consent_record()
            .find_many()
            .where_expr(
                FilterExpr::from(consent_record::appId().eq(app_id.to_owned()))
                    .and(consent_record::msisdnHash().eq(msisdn_hash))
                    .and(consent_record::scope().eq(class)),
            )
            .limit(1)
            .run(sys)
            .await?;
        if consented.is_empty() {
            Err(CratestackError::Validation(format!(
                "no consent record on file for this recipient for {class:?} messages — \
                 Law No. 2024/017 requires opt-in consent before this class may be sent"
            )))
        } else {
            Ok(())
        }
    }

    /// §3.2 step: `App.monthlyQuota`, counted over the current UTC calendar
    /// month.
    ///
    /// A soft cap, not a hard one: this count-then-`send()`-creates check is
    /// a TOCTOU race across two separate operations, so N concurrent sends
    /// for the same app that all pass this check can land the app at
    /// `monthlyQuota + N - 1`. Flagged in review (#94), accepted as-is —
    /// closing it needs either the count-and-create in one transaction or a
    /// database-level check constraint, and nothing in §3.2 asks for a hard
    /// cap precise to the message. Revisit if a real customer relies on
    /// the quota as an exact ceiling rather than a monthly budget signal.
    async fn ensure_within_quota(
        db: &schema::IsolatedCratestack,
        sys: &CratestackContext,
        app: &schema::App,
        now: DateTime<Utc>,
    ) -> Result<(), CratestackError> {
        let sent_this_month = db
            .message()
            .aggregate()
            .count()
            .where_expr(
                FilterExpr::from(message::appId().eq(app.id.clone()))
                    .and(message::createdAt().gte(month_start(now))),
            )
            .run(sys)
            .await?;

        if sent_this_month < app.monthlyQuota {
            Ok(())
        } else {
            Err(CratestackError::Validation(format!(
                "monthly quota of {} messages exceeded",
                app.monthlyQuota
            )))
        }
    }

    /// §3.2 step: resolve a sender ID (explicit, or the app's default),
    /// requiring an active `SenderId` with at least one approved
    /// `SenderIdRegistration`.
    ///
    /// `"approved"` is this procedure's own convention for
    /// `SenderIdRegistration.status` — a plain `String` in the schema, not
    /// an enum (§2.0: nothing here enforces it at the database level).
    /// Whatever eventually writes approvals (an admin action, a provider
    /// webhook) has to use this exact string, or every registration will
    /// silently read as unapproved.
    async fn resolve_sender_id(
        db: &schema::IsolatedCratestack,
        sys: &CratestackContext,
        app: &schema::App,
        requested: Option<&str>,
    ) -> Result<String, CratestackError> {
        const APPROVED: &str = "approved";

        let value = if let Some(value) = requested {
            value.to_owned()
        } else {
            let default_id = app.defaultSenderIdId.clone().ok_or_else(|| {
                CratestackError::Validation(
                    "no senderId given and this app has no default sender".to_owned(),
                )
            })?;
            db.sender_id()
                .find_many()
                .where_expr(FilterExpr::from(sender_id::id().eq(default_id)))
                .limit(1)
                .run(sys)
                .await?
                .into_iter()
                .next()
                .ok_or_else(|| {
                    CratestackError::Validation(
                        "this app's default sender id no longer exists".to_owned(),
                    )
                })?
                .value
        };

        let sender = db
            .sender_id()
            .find_many()
            .where_expr(
                FilterExpr::from(sender_id::value().eq(value.clone()))
                    .and(sender_id::active().is_true()),
            )
            .limit(1)
            .run(sys)
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| {
                CratestackError::Validation(format!(
                    "sender id {value:?} is not a registered sender"
                ))
            })?;

        let approved = db
            .sender_id_registration()
            .find_many()
            .where_expr(
                FilterExpr::from(sender_id_registration::senderIdId().eq(sender.id))
                    .and(sender_id_registration::status().eq(APPROVED)),
            )
            .limit(1)
            .run(sys)
            .await?;

        if approved.is_empty() {
            Err(CratestackError::Validation(format!(
                "sender id {value:?} has no approved provider registration"
            )))
        } else {
            Ok(value)
        }
    }

    /// A best-effort quote for `SendMessageResult.estimatedCostXaf`: the
    /// cheapest active provider's per-segment cost, times segments.
    ///
    /// Deliberately not real routing (§6.3) — no `Route` rule evaluation,
    /// no capability filtering, no operator-specific pricing (on-net vs.
    /// all-operator, §6.2). This is the number quoted at accept time,
    /// before a route is ever chosen; the message's actual `costXaf`,
    /// stamped by the worker at submission, is the number that bills.
    /// `Decimal::ZERO` when no active provider exists yet — honest given
    /// nothing is configured, not a fabricated estimate.
    async fn estimate_cost(
        db: &schema::IsolatedCratestack,
        sys: &CratestackContext,
        segments: i64,
    ) -> Result<Decimal, CratestackError> {
        let cheapest = db
            .provider()
            .find_many()
            .where_expr(FilterExpr::from(
                provider::state().eq(schema::ProviderState::active),
            ))
            .order_by(provider::costPerSegmentXaf().asc())
            .limit(1)
            .run(sys)
            .await?;

        let per_segment = cheapest
            .into_iter()
            .next()
            .map_or(Decimal::ZERO, |p| p.costPerSegmentXaf);

        Ok(per_segment * Decimal::from(segments))
    }

    /// The caller's own `client_id`, from the authenticated context's
    /// `sub` claim — refusing anything that isn't a machine caller.
    ///
    /// A human (`kind == "user"`) principal has no `appId` to derive one
    /// from at all: §4.2/§5's design leaves `Principal.app_id` empty for
    /// every human caller, and `SendMessageInput` carries no explicit
    /// `appId` field either. The schema's own `@allow` on `sendMessage`
    /// admits `owner`/`admin`/`operator` roles too, so this is a real gap,
    /// not a theoretical one — surfaced as a clear error rather than
    /// guessed at, until there's a design for which app a human sends on
    /// behalf of.
    fn caller_client_id(ctx: &CratestackContext) -> Result<String, CratestackError> {
        let kind = match ctx.auth_field("kind") {
            Some(Value::String(kind)) => kind.as_str(),
            _ => {
                return Err(CratestackError::Unauthorized(
                    "missing kind claim".to_owned(),
                ));
            }
        };
        if kind != PrincipalKind::App.as_str() {
            return Err(CratestackError::Validation(
                "sendMessage currently requires a machine (registered client key) caller — \
                 deriving an App for a human caller has no design yet"
                    .to_owned(),
            ));
        }
        match ctx.auth_field("sub") {
            Some(Value::String(sub)) => Ok(sub.clone()),
            _ => Err(CratestackError::Unauthorized(
                "missing sub claim".to_owned(),
            )),
        }
    }

    /// §3.2/§10/#72: the pre-persistence steps. A message that reaches the
    /// database is one already decided to be sendable — this procedure's
    /// job is that decision, not delivery.
    async fn send(
        &self,
        db: &schema::IsolatedCratestack,
        ctx: &CratestackContext,
        args: schema::SendMessageInput,
    ) -> Result<schema::SendMessageResult, CratestackError> {
        // Layer 2 (#24, §5.1), checked before anything else runs. Layer 1's
        // own `@allow` on `sendMessage` admits any authenticated
        // `kind == "app"` caller unconditionally (schema.cstack:
        // `auth().kind == "app" || hasRole('owner') || hasRole('admin') ||
        // hasRole('operator')`) — nothing about *role* narrows a machine
        // caller down further, so this is the only thing standing between
        // "any registered client key" and "sends SMS", per §5.2's service-account scope table (`sms:send`).
        require_permission(ctx, "sms:send")?;

        let sys = Self::sys();
        let now = Utc::now();

        // 1. client_id -> App (cached).
        let client_id = Self::caller_client_id(ctx)?;
        let app = self.resolve_app(db, &sys, client_id).await?;

        // 2. MSISDN normalisation. `parse_mobile` rejects fixed-line and
        // unallocated ranges — failing here beats failing on a DLR later.
        let msisdn = Msisdn::parse_mobile(&args.to)
            .map_err(|error| CratestackError::Validation(error.to_string()))?;
        let msisdn_hash = self.keyed_hash_hex(msisdn.as_e164());

        // #72: classification decided here, ahead of the checks that key
        // off it, rather than at message-construction time further down
        // (where it used to live, unused until then) — `crate::consent`'s
        // own module doc is the full reasoning for what each of the next
        // three steps does and does not prove.
        let class = args.class.unwrap_or(schema::MessageClass::transactional);

        // 3. Opt-out check, before anything else persists. Class-gated
        // since #72 (see `ensure_not_opted_out`'s own doc).
        if crate::consent::requires_recipient_consent_controls(class) {
            Self::ensure_not_opted_out(db, &sys, &msisdn_hash).await?;
        }

        // 3a. #72: self-imposed marketing quiet hours (08:00-20:00 WAT).
        // `notification` deliberately does not gate here — see
        // `consent::subject_to_quiet_hours`'s own doc.
        if crate::consent::subject_to_quiet_hours(class) {
            // The delivery time, not the accept time — a scheduled send is
            // what the recipient actually experiences. See the function's
            // own doc for the bypass this closes.
            Self::ensure_within_marketing_quiet_hours(args.scheduledAt.unwrap_or(now))?;
        }

        // 3b. #72: consent on file, for the classes that require it. Reads
        // under `sys`, so it runs regardless of what the caller can see —
        // the same reason `ensure_not_opted_out` already does.
        if crate::consent::requires_recipient_consent_controls(class) {
            Self::ensure_consent_on_file(db, &sys, &app.id, &msisdn_hash, class).await?;
        }

        // 4. Quota.
        Self::ensure_within_quota(db, &sys, &app, now).await?;

        // 5. Sender ID resolution + approval.
        let sender_id_value =
            Self::resolve_sender_id(db, &sys, &app, args.senderId.as_deref()).await?;

        // 6. Encoding analysis, on the body that will actually be sent —
        // normalised unconditionally, transliterated only if this app
        // opted in (§2.2: transliteration is perceptible and never
        // implicit).
        let normalised = normalise(&args.body);
        let body = if app.transliterateToGsm7 {
            transliterate_to_gsm7(&normalised).0
        } else {
            normalised
        };
        let report = analyse(&body);

        // 7. Operator classification (routing hint only, per sms-msisdn's
        // own doc — never load-bearing).
        let operator = self
            .classify_operator(db.operator_prefix_rule(), &sys, &msisdn)
            .await?;

        // 8. Idempotency: the DB-level defence described in §4.5 as
        // independent of the HTTP `Idempotency-Key` layer (#153,
        // `router.rs`'s `IdempotencyLayer` — a real, mounted Tower layer
        // as of #153, not a hypothetical one) — that layer wraps the whole
        // router and never reaches procedure code at all, so this DB-level
        // check runs regardless of whether a caller sent an
        // `Idempotency-Key`. `clientRef` is the only caller-supplied
        // correlation string `SendMessageInput` carries, so it doubles as
        // `idempotencyKey` when present; a caller that supplies neither
        // gets no DB-level dedupe, the same way a caller that skips the
        // `Idempotency-Key` header gets no HTTP-level replay protection
        // (§4.5: "document loudly"). The two are independent, complementary
        // defences, not a primary and a decoy — see §4.5 for which failure
        // mode each one alone leaves open.
        let idempotency_key = args.clientRef.clone();

        // 9. Cost estimate for the response — see `estimate_cost`'s own
        // doc for why this is a quote, not a routing decision.
        let segments = i64::from(report.segments);
        let estimated_cost = Self::estimate_cost(db, &sys, segments).await?;

        // `class` was already decided at step 3, ahead of the checks that
        // key off it.
        let validity = args
            .validityMinutes
            .map_or_else(|| default_validity(class), ChronoDuration::minutes);
        let expires_at = args.scheduledAt.unwrap_or(now) + validity;

        // #134: peppered under the same scheme as `msisdnHash`. A
        // templated OTP body ("Votre code est 4821") is low-entropy and
        // enumerable by the exact same brute-force argument the issue makes
        // for MSISDNs — grepped for every reader of `bodyHash` before
        // deciding this (`rg bodyHash`, this crate and the admin console's
        // generated TS client): it is write-only, never read back or
        // cross-compared against a value computed outside this system, so
        // there is no external party this would break by keying it. Pepper
        // it too rather than leave a second, weaker hash in the same row.
        let body_hash = self.keyed_hash_hex(&body);
        let body_len = i64::try_from(body.len()).unwrap_or(i64::MAX);

        // create()'s own @@emit(created, updated) writes the transactional
        // outbox row as a side effect — no separate outbox insert here.
        let message = db
            .message()
            .create(schema::CreateMessageInput {
                appId: app.id,
                clientRef: args.clientRef,
                idempotencyKey: idempotency_key,
                msisdn: msisdn.into_e164(),
                msisdnHash: msisdn_hash,
                operator,
                senderIdValue: sender_id_value,
                class,
                priority: 100,
                body: Some(body),
                bodyHash: body_hash,
                bodyLength: body_len,
                encoding: encoding_of(report.encoding),
                segments,
                stateReason: None,
                routeId: None,
                providerId: None,
                providerMessageRef: None,
                providerMessageRefAlt: None,
                excludedRouteIds: None,
                maxAttempts: 3,
                leaseOwner: None,
                leaseUntil: None,
                scheduledAt: args.scheduledAt,
                expiresAt: expires_at,
                submittedAt: None,
                finalizedAt: None,
                // #67: absent until `purge_retention` reaches this row past
                // its 90-day retention window.
                purgedAt: None,
            })
            .run(&sys)
            .await?;

        // #71: the correlation event — see this module's own doc. Logged
        // at `info`, not `debug`: this is the one line that lets an
        // operator go from "a customer says this OTP never arrived" to
        // "grep this message_id across both processes' logs" without
        // already knowing which worker or which tick handled it.
        info!(
            message_id = %message.id,
            app_id = %message.appId,
            client_ref = message.clientRef.as_deref().unwrap_or(""),
            cratestack_request_id = ctx.request_id().unwrap_or(""),
            state = ?message.state,
            "message accepted"
        );

        Ok(schema::SendMessageResult {
            messageId: message.id,
            state: message.state,
            encoding: message.encoding,
            segments: message.segments,
            operator: message.operator,
            estimatedCostXaf: estimated_cost,
        })
    }

    /// #23, reshaped by VS-4: mint a fresh `AppClient` and register the
    /// caller's Ed25519 public key for it, in one transaction. The caller
    /// generated the keypair and sends only the public half
    /// (`args.publicKey`, the same encoding `registerAppClientKey` takes), so
    /// nothing is generated here and nothing secret is returned: vsms never
    /// holds a private key, which is the whole signed-transport design. It
    /// used to generate an RSA keypair and hand back the private half once,
    /// for a `private_key_jwt` token server that no longer exists.
    ///
    /// Both rows are written in one `serializable` transaction (the
    /// procedure's own `@isolation`): `ClientKey.appClientId` references
    /// `AppClient.id`, and a caller must never observe a client without its
    /// key or the reverse — either the client is fully provisioned or it does
    /// not exist yet. A thumbprint that is already registered (to this client
    /// or another) is a `Conflict`: a new client cannot adopt a key that is
    /// somebody's already.
    ///
    /// **Retirement and rotation are the existing procedures' job.**
    /// `registerAppClientKey` adds a second key to a client and
    /// `revokeAppClientKey` ends the old one (an overlap window, since both
    /// verify until the old is revoked); `AppClient.active`/`retiredAt`
    /// through the generated `update` route ends the whole client.
    async fn provision_client(
        &self,
        db: &schema::IsolatedCratestack,
        _ctx: &CratestackContext,
        args: schema::ProvisionClientInput,
    ) -> Result<schema::ProvisionClientResult, CratestackError> {
        let sys = Self::sys();

        let scopes_packed =
            pack(&args.scopes).map_err(|error| CratestackError::Validation(error.to_string()))?;
        // `console:pair` lets a client attach a console key to any linked
        // person, so an owner or admin handing it out over HTTP is a way to
        // become someone else. It is granted by `sms-gateway register-client`.
        // Refused before the app is read or a key is looked at.
        crate::pair_scope::refuse_pair_scope(&scopes_packed)?;
        // Before the database, so a typo'd key fails with nothing touched.
        let parsed = crate::client_keys::parse_public_key(&args.publicKey)?;

        // The app must exist and be active before a client is allowed to
        // speak for it — provisioning a client for a deleted/deactivated
        // app would silently mint a working credential for nothing.
        let app = db
            .app()
            .find_many()
            .where_expr(FilterExpr::from(app::id().eq(args.appId.clone())))
            .limit(1)
            .run(&sys)
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| CratestackError::NotFound(format!("no App with id {}", args.appId)))?;
        if !app.active {
            return Err(CratestackError::Validation(format!(
                "App {} is not active",
                app.id
            )));
        }

        // clientId is caller-visible immediately (it is what a signed
        // request resolves to), so it is generated here rather than left to a
        // DB default — a `Cuid` default would also exclude it from
        // CreateXInput entirely (§2.0). Not a `Cuid` itself:
        // `AppClient.clientId` is a plain `String @length(min: 8, max: 64)`,
        // so there is no `[a-z0-9]{2,32}` format guard to satisfy.
        let client_id = format!("appc_{}", cratestack::uuid::Uuid::new_v4().simple());

        let app_id = args.appId;
        let label = args.label;

        let key = db
            .transaction(async |tx| {
                let sys = &sys;
                let thumbprint_taken = !db
                    .client_key()
                    .find_many()
                    .where_expr(FilterExpr::from(
                        client_key::thumbprint().eq(parsed.thumbprint.clone()),
                    ))
                    .limit(1)
                    .run_in_tx(tx, sys)
                    .await?
                    .is_empty();
                if thumbprint_taken {
                    return Err(CratestackError::Conflict(
                        "this public key is already registered to a client".to_owned(),
                    ));
                }

                let app_client = db
                    .app_client()
                    .create(schema::CreateAppClientInput {
                        appId: app_id,
                        clientId: client_id.clone(),
                        label: label.clone(),
                        scopes: scopes_packed,
                        lastUsedAt: None,
                        retiredAt: None,
                    })
                    .run_in_tx(tx, sys)
                    .await?
                    // cratestack 0.7.13 (#554): `run_in_tx` write builders
                    // now return `RunInTxOutcome<T>` (the value plus any
                    // `AuditEvent`s built for a caller-installed `AuditSink`
                    // to fan out after commit). This codebase installs no
                    // `AuditSink` — `@@audit` rows still land in
                    // `cratestack_audit` the way they always have, inside
                    // this same transaction, regardless — so `.value` is
                    // the only thing any call site here needs.
                    .value;

                let key = db
                    .client_key()
                    .create(schema::CreateClientKeyInput {
                        appClientId: app_client.id,
                        alg: schema::ClientKeyAlg::ed25519,
                        publicKey: parsed.bytes.to_vec(),
                        thumbprint: parsed.thumbprint.clone(),
                        label: Some(label),
                        revokedAt: None,
                    })
                    .run_in_tx(tx, sys)
                    .await?
                    .value;
                Ok(key)
            })
            .await
            .map_err(map_database_error)?;

        info!(
            client_id = %client_id,
            key_id = %key.id,
            thumbprint = %key.thumbprint,
            "app client provisioned"
        );
        Ok(schema::ProvisionClientResult {
            clientId: client_id,
            keyId: key.id,
            thumbprint: key.thumbprint,
        })
    }

    /// #41: rotate `WebhookEndpoint.secret` with a 24-hour overlap window
    /// — the current `secret` moves to `prevSecret` (nothing is thrown
    /// away; the endpoint keeps verifying signatures made with its old
    /// secret until `prevSecret` is cleared) and a fresh
    /// `sms_webhook::generate_secret()` value takes its place.
    /// `secretRotatedAt` records when this happened.
    ///
    /// **Clearing `prevSecret` after 24 hours is out of scope for this
    /// procedure.** §4.4's own words: "a job clears `prevSecret` after 24
    /// hours" — that's §7.5's `cleanup_secrets` job, one of the eight job
    /// kinds this repo's own M2 status notes (`AGENTS.md`, `#35`) already
    /// lists as not yet built, for the same reason every other one isn't:
    /// no `jobs` role infrastructure gap here, just no story that wires
    /// this specific job kind up yet. Until it exists, an operator who
    /// rotates a secret is responsible for knowing when 24 hours have
    /// passed and, if they want the old secret to stop working sooner,
    /// rotating a second time (which overwrites `prevSecret` again).
    ///
    /// **Update, #59:** `WebhookEndpoint` gained `@version`, and the write
    /// below now carries `if_match(endpoint.version)` too. This procedure's
    /// own `@isolation("serializable")` transaction is still the primary
    /// guard against two concurrent rotations of the same endpoint racing
    /// — Postgres aborts the second transaction with a serialization
    /// failure rather than silently letting one rotation's write clobber
    /// the other's `prevSecret` — `if_match` is additional, not a
    /// replacement: it turns the same race into a named
    /// `PreconditionFailed` for a caller that reaches this procedure with
    /// a stale `ETag` from a prior read, rather than relying solely on a
    /// database-level serialization-failure retry the caller has no
    /// visibility into.
    ///
    /// **VS-8R: a `sealed` endpoint has no secret and is refused**
    /// (`CratestackError::Validation`, 422) before anything is written: there
    /// is nothing to rotate, and minting one would hand the endpoint a
    /// shared secret its mode exists to not have (the CHECK
    /// `webhook_endpoints_delivery_mode_sealed_holds_no_secret_check` would
    /// refuse the write too; this is the answer that tells the caller why).
    /// An `hmac` endpoint rotates exactly as before.
    ///
    /// Reads and writes as `sys`, matching every other procedure's write
    /// path in this file (see `Procedures::sys`'s own doc) — not the
    /// caller's own `ctx`, even though `ctx`'s owner/frontends/apps/admin/developer role
    /// already satisfies `WebhookEndpoint`'s `read`/`update` policy on its
    /// own. `WebhookEndpoint` had no `hasRole('system')` clause until this
    /// change — the schema's own comment on that clause explains why (no
    /// internal system-context reader existed before this procedure), and
    /// it is the eighth instance of the exact shape `AGENTS.md`'s
    /// "Invariants that fail the build rather than production" section
    /// already names seven times over. See that file and
    /// `backends/crates/sms-api/tests/system_context_golden_list_live_postgres.rs`.
    ///
    /// #193: calls `require_permission(ctx, "webhook:manage")` (Layer 2,
    /// §5.1) before touching anything — `replayWebhookAttempt` (#43, #191)
    /// already did, and rotation is the *more* sensitive of the two
    /// operations (it changes the credential every future delivery is
    /// signed with and starts the `prevSecret` overlap clock), so it made
    /// no sense to be the one left ungated. The issue's own alternative —
    /// decide Layer 2 is redundant given `WebhookEndpoint.update`'s
    /// already-role-scoped Layer 1, and strike `webhook:manage` from
    /// §5.2's vocabulary instead — was rejected: a permission that appears
    /// in the role table and is never checked anywhere is worse than no
    /// permission, since it implies a control that doesn't exist, and
    /// `replayWebhookAttempt` already relies on it being real. Uses `ctx`
    /// (the caller's own context, for the permission check only) rather
    /// than the `_ctx` this function used to ignore; every read/write
    /// below still goes through `sys`, unaffected by this change. Same
    /// latency as #187: before VS-3 no credential carried a human role
    /// (#97/#98's scope cut), so this check had no live *allow* path to
    /// prove — only *deny*, covered by `rotate_webhook_secret_live_postgres.rs`'s
    /// `rotate_denies_a_caller_with_no_webhook_manage_permission`.
    async fn rotate_secret(
        &self,
        db: &schema::IsolatedCratestack,
        ctx: &CratestackContext,
        args: schema::EndpointInput,
    ) -> Result<schema::WebhookEndpoint, CratestackError> {
        require_permission(ctx, "webhook:manage")?;
        // VSA-1: before anything is read, so the answer does not depend on
        // whether the endpoint exists or which mode it is in.
        self.hmac_webhooks
            .require("a signing secret cannot be rotated (rotateWebhookSecret)")?;

        let sys = Self::sys();
        let endpoint_id = args.endpointId;

        db.transaction(async |tx| {
            let sys = &sys;
            let endpoint = db
                .webhook_endpoint()
                .find_many()
                .where_expr(FilterExpr::from(
                    webhook_endpoint::id().eq(endpoint_id.clone()),
                ))
                .limit(1)
                .run_in_tx(tx, sys)
                .await?
                .into_iter()
                .next()
                .ok_or_else(|| {
                    CratestackError::NotFound(format!("no WebhookEndpoint with id {endpoint_id}"))
                })?;

            if endpoint.deliveryMode != schema::WebhookDeliveryMode::hmac {
                return Err(CratestackError::Validation(format!(
                    "webhook endpoint {endpoint_id} is sealed: it has no signing secret to \
                     rotate (its deliveries are sealed under vsms's envelope signing key)"
                )));
            }

            let updated = db
                .webhook_endpoint()
                .update(endpoint_id)
                .set(schema::UpdateWebhookEndpointInput {
                    secret: Some(Some(sms_webhook::generate_secret())),
                    prevSecret: Some(endpoint.secret),
                    secretRotatedAt: Some(Some(Utc::now())),
                    ..Default::default()
                })
                // #59: WebhookEndpoint gained `@version`. `@isolation
                // ("serializable")` above already stops two concurrent
                // rotations of the same row from clobbering each
                // other's `prevSecret` — this doc comment's own
                // reasoning predates the field existing — so
                // `if_match` here is defense in depth, not the primary
                // guard: it makes a losing race a named
                // `PreconditionFailed` instead of relying solely on a
                // serialization-failure retry.
                .if_match(endpoint.version)
                .run_in_tx(tx, sys)
                .await?
                // cratestack 0.7.13 (#554): see the identical comment on
                // `provisionAppClient`'s own `run_in_tx` call above.
                .value;

            Ok(updated)
        })
        .await
    }

    /// VS-8R: change an endpoint's delivery mode, validated, in one serializable
    /// transaction.
    ///
    /// A raw `PATCH` of `deliveryMode` has to move four columns in step, and one
    /// left out breaks a CHECK of `0010_sealed_webhooks`, which the generated
    /// routes answer with a database error (`500`). This answers `422` with the
    /// reason instead, and does the bookkeeping itself ([`plan_mode_switch`]):
    ///
    /// - **to `hmac`** it mints the secret server-side (`whsec_` and 64 hex
    ///   characters, in the returned row as a create's response carries it, for
    ///   the receiver to be given) and frees the audience, which `hmac` has none
    ///   of. An `audience` in the request is refused.
    /// - **to `sealed`** it requires an `audience` (valid as the column holds
    ///   it: 1 to 255 characters, never `vsms`, unique: a taken one is `409`)
    ///   and a URL a delivery can be sealed for, and clears `secret`,
    ///   `prevSecret` and `secretRotatedAt`.
    ///
    /// Switching to the mode the endpoint is in is `422` too: there is nothing
    /// to switch, and `rotateWebhookSecret` or a `PATCH` of the audience is what
    /// the caller wants.
    ///
    /// Same shape as [`Self::rotate_secret`]: `webhook:manage` (Layer 2) first,
    /// the row read and written as `sys`, its version carried as `if_match`.
    async fn switch_mode(
        &self,
        db: &schema::IsolatedCratestack,
        ctx: &CratestackContext,
        args: schema::SwitchWebhookDeliveryModeInput,
    ) -> Result<schema::WebhookEndpoint, CratestackError> {
        require_permission(ctx, "webhook:manage")?;

        let sys = Self::sys();
        let schema::SwitchWebhookDeliveryModeInput {
            endpointId: endpoint_id,
            mode,
            audience,
        } = args;
        // VSA-1: a switch to `sealed` is the way out of `hmac` and is always
        // allowed; a switch to `hmac` mints a secret and is not.
        if mode == schema::WebhookDeliveryMode::hmac {
            self.hmac_webhooks
                .require("an endpoint cannot be switched to hmac (switchWebhookDeliveryMode)")?;
        }

        db.transaction(async |tx| {
            let sys = &sys;
            let endpoint = db
                .webhook_endpoint()
                .find_many()
                .where_expr(FilterExpr::from(
                    webhook_endpoint::id().eq(endpoint_id.clone()),
                ))
                .limit(1)
                .run_in_tx(tx, sys)
                .await?
                .into_iter()
                .next()
                .ok_or_else(|| {
                    CratestackError::NotFound(format!("no WebhookEndpoint with id {endpoint_id}"))
                })?;

            let change = plan_mode_switch(&endpoint, mode, audience.as_deref())?;

            let updated = db
                .webhook_endpoint()
                .update(endpoint_id.clone())
                .set(change)
                .if_match(endpoint.version)
                .run_in_tx(tx, sys)
                .await
                .map_err(map_database_error)?
                .value;

            Ok(updated)
        })
        .await
    }

    /// #43: re-fire a stuck `WebhookAttempt` from the admin surface — an
    /// operator's explicit "I fixed the receiving end, try again" action.
    /// See §8.5's own "Implementation, #43" note in the design doc for the
    /// full design reasoning (the state-machine edges, why `succeeded`
    /// stays out of reach, and the circuit-breaker call); this doc comment
    /// only covers what isn't already there.
    ///
    /// `webhook_attempts_dedupe` (`endpoint_id`, `aggregate_id`,
    /// `event_type`) means a second row for this event can never exist, so
    /// replay resets the existing row rather than creating a new one —
    /// which is also why the envelope's `id` (`WebhookAttempt.id`) and
    /// `sourceEventId` are both preserved automatically, with nothing here
    /// needing to say so explicitly.
    ///
    /// Only `failed`/`dead` are replayable. The match below is Rust
    /// proposing only the two edges `attempt_state_transitions` (§2.10)
    /// actually admits (`failed -> pending`, `dead -> pending`) rather than
    /// leaning on the trigger to reject a `pending`/`delivering`/`succeeded`
    /// attempt after the fact — a clearer, named error for the expected-
    /// usage case. The trigger stays the backstop against a state change
    /// racing this read (e.g. `hooks` claiming the same `failed` row
    /// between this function's read and write): `if_match(attempt.version)`
    /// turns that race into `PreconditionFailed`, not a corrupted attempt.
    ///
    /// Also resets the endpoint's circuit-breaker bookkeeping
    /// (`consecutiveFailures`/`circuitOpenUntil`) when either is set — an
    /// operator explicitly replaying is the exact signal the breaker exists
    /// to wait for. Scoped to the *endpoint*, not just this attempt, on
    /// purpose: every other `pending`/`failed` row against the same
    /// endpoint was equally stuck behind the same stale breaker, and a
    /// per-attempt bypass would need `claim.rs`'s shared candidate filter
    /// to grow a special case for one procedure's benefit. Deliberately
    /// never touches `WebhookEndpoint.active` — reactivating a deactivated
    /// endpoint (e.g. after a 410 Gone) is a separate, more consequential
    /// decision than retrying one delivery, left to the existing
    /// `PATCH /webhook_endpoints/{id}` route.
    async fn replay_attempt(
        &self,
        db: &schema::IsolatedCratestack,
        ctx: &CratestackContext,
        args: schema::ReplayWebhookAttemptInput,
    ) -> Result<schema::WebhookAttempt, CratestackError> {
        require_permission(ctx, "webhook:manage")?;

        let sys = Self::sys();
        let attempt_id = args.attemptId;
        let now = Utc::now();

        db.transaction(async |tx| {
            let sys = &sys;
            let attempt = db
                .webhook_attempt()
                .find_many()
                .where_expr(FilterExpr::from(
                    webhook_attempt::id().eq(attempt_id.clone()),
                ))
                .limit(1)
                .run_in_tx(tx, sys)
                .await?
                .into_iter()
                .next()
                .ok_or_else(|| {
                    CratestackError::NotFound(format!("no WebhookAttempt with id {attempt_id}"))
                })?;

            match attempt.state {
                schema::AttemptState::failed | schema::AttemptState::dead => {}
                other => {
                    return Err(CratestackError::Conflict(format!(
                        "webhook attempt {attempt_id} is {other:?}; replay only applies to \
                             a failed or dead delivery"
                    )));
                }
            }

            let endpoint_id = attempt.endpointId.clone();

            let updated = db
                .webhook_attempt()
                .update(attempt_id.clone())
                .set(schema::UpdateWebhookAttemptInput {
                    state: Some(schema::AttemptState::pending),
                    attempts: Some(0),
                    lastStatusCode: Some(None),
                    lastError: Some(None),
                    leaseOwner: Some(None),
                    leaseUntil: Some(None),
                    nextAttemptAt: Some(Some(now)),
                    ..Default::default()
                })
                .if_match(attempt.version)
                .run_in_tx(tx, sys)
                .await?
                // cratestack 0.7.13 (#554): see the identical comment on
                // `provisionAppClient`'s own `run_in_tx` call above.
                .value;

            let endpoint = db
                .webhook_endpoint()
                .find_many()
                .where_expr(FilterExpr::from(webhook_endpoint::id().eq(endpoint_id)))
                .limit(1)
                .run_in_tx(tx, sys)
                .await?
                .into_iter()
                .next();

            if let Some(endpoint) = endpoint
                && (endpoint.consecutiveFailures != 0 || endpoint.circuitOpenUntil.is_some())
            {
                // #59: if_match(endpoint.version) — the row was
                // just read above, inside this same transaction, so
                // the version is fresh. A losing race here (another
                // writer touched this endpoint between the read and
                // this write) surfaces as PreconditionFailed and
                // aborts the whole replay rather than silently
                // clobbering whatever the other writer just set.
                let endpoint_version = endpoint.version;
                db.webhook_endpoint()
                    .update(endpoint.id)
                    .set(schema::UpdateWebhookEndpointInput {
                        consecutiveFailures: Some(0),
                        circuitOpenUntil: Some(None),
                        ..Default::default()
                    })
                    .if_match(endpoint_version)
                    .run_in_tx(tx, sys)
                    .await?;
            }

            Ok(updated)
        })
        .await
        // The one write above that can hit an illegal edge (`webhook_attempt`'s
        // own `update`) is inside this same `Result`, so mapping once here
        // catches it — an application-level state-check `Conflict` from the
        // match above carries no `db_sqlstate`, so this is a no-op pass-
        // through for that case, per `map_database_error`'s own contract.
        .map_err(map_database_error)
    }

    /// #56: re-enqueue a `dead` `Job` from the admin surface — the same
    /// "reset the existing row rather than creating a new one" shape
    /// [`Self::replay_attempt`] (#43) established for `WebhookAttempt`,
    /// adapted to `Job`'s own state machine.
    ///
    /// Only `dead` is accepted. `failed -> pending` already exists in
    /// `job_state_transitions` — `backends/crates/sms-worker/src/jobs.rs`'s own
    /// `apply_failure` uses it for automatic backoff — but `failed` is a
    /// same-tick transient state: `apply_failure` writes `running -> failed`
    /// and then, within the same function call, immediately writes
    /// `failed -> {pending, dead}`. No operator poll can realistically catch
    /// a job sitting in `failed` between those two writes, so building a
    /// button against that state would be dead code exercising a window
    /// that closes in milliseconds. `dead -> pending` is the edge this
    /// procedure actually needs and is new (§2.10's table, `0002_bootstrap`
    /// regenerated) — `dead` had no outgoing edges before this PR, on
    /// purpose (`0002_bootstrap`'s own comment: "succeeded, dead, cancelled
    /// are terminal"), and this procedure is the only place that changes.
    ///
    /// Resets `attempts` to 0 and clears `lastError`/`leaseOwner`/
    /// `leaseUntil`, giving the job a fresh run at its full `maxAttempts`
    /// budget rather than resuming a counter that was already exhausted —
    /// otherwise `jobs::apply_failure` would send it straight back to `dead`
    /// on its very next failure. `runAt` is set to now so the next `jobs`
    /// poll picks it up immediately rather than waiting on whatever
    /// `runAt` the original enqueue left behind.
    async fn requeue(
        &self,
        db: &schema::IsolatedCratestack,
        ctx: &CratestackContext,
        args: schema::RequeueJobInput,
    ) -> Result<schema::Job, CratestackError> {
        require_permission(ctx, "job:enqueue")?;

        let sys = Self::sys();
        let job_id = args.jobId;
        let now = Utc::now();

        db.transaction(async |tx| {
            let sys = &sys;
            let existing = db
                .job()
                .find_many()
                .where_expr(FilterExpr::from(job::id().eq(job_id.clone())))
                .limit(1)
                .run_in_tx(tx, sys)
                .await?
                .into_iter()
                .next()
                .ok_or_else(|| CratestackError::NotFound(format!("no Job with id {job_id}")))?;

            if existing.state != schema::JobState::dead {
                return Err(CratestackError::Conflict(format!(
                    "job {job_id} is {:?}; requeue only applies to a dead job",
                    existing.state
                )));
            }

            let updated = db
                .job()
                .update(job_id.clone())
                .set(schema::UpdateJobInput {
                    state: Some(schema::JobState::pending),
                    attempts: Some(0),
                    lastError: Some(None),
                    leaseOwner: Some(None),
                    leaseUntil: Some(None),
                    runAt: Some(now),
                    ..Default::default()
                })
                .if_match(existing.version)
                .run_in_tx(tx, sys)
                .await?
                // cratestack 0.7.13 (#554): see the identical comment on
                // `provisionAppClient`'s own `run_in_tx` call above.
                .value;

            Ok(updated)
        })
        .await
        // Same reasoning as `replay_attempt`'s own trailing `map_err`: the
        // one write above that can hit an illegal edge is inside this same
        // `Result`, so mapping once here catches a genuine `SM001` race
        // (another caller flipping this row's state between the read above
        // and this write) without disturbing the application-level
        // `Conflict` the match above already returns for the expected
        // "not dead" case, which carries no `db_sqlstate` and passes
        // through unchanged.
        .map_err(map_database_error)
    }

    /// #57: which node holds which singleton-role advisory lock — see
    /// `worker_locks.rs`'s own module doc for the query, what was verified
    /// live against a real Postgres about what `pg_locks` actually reports
    /// for a session advisory lock, and why "two dispatch workers" can
    /// never show up as two granted rows for one role.
    async fn worker_lock_snapshot(
        &self,
        db: &schema::Cratestack,
        ctx: &CratestackContext,
    ) -> Result<schema::WorkerLocksResult, CratestackError> {
        require_permission(ctx, "worker:read")?;

        let locks = worker_locks::current_locks(db).await?;
        Ok(schema::WorkerLocksResult { locks })
    }

    /// #54: "given this recipient, class and app, which route wins and
    /// why" — without sending anything. Reads under `sys()`, the same
    /// reason `sendMessage`'s own `classify_operator`/routing-adjacent
    /// reads do: `Route`/`Provider`'s own `@@allow("read", ...)` doesn't
    /// need to admit this procedure's caller directly (it already admits
    /// `hasRole('system')`), only `Route.read`/`Provider.read`'s new
    /// `auth().kind == "app"` clause (this PR) needs to, and that's for the
    /// plain `GET /providers`/`GET /routes` list screens, not this
    /// procedure.
    ///
    /// `require_permission(ctx, "route:read")` is the real perimeter for
    /// the admin console's own machine credential today, same shape as
    /// `worker_lock_snapshot` above and `#56`/`#57`'s own `job:read`/
    /// `worker:read` precedent: `simulateRoute`'s own `@allow` admits any
    /// `auth().kind == "app"` caller unconditionally (no `appId` on
    /// `Route`/`Provider` to scope a row-level predicate by), so only a
    /// granted `route:read` scope stands between a provisioned app client
    /// and this procedure.
    async fn simulate(
        &self,
        db: &schema::Cratestack,
        ctx: &CratestackContext,
        args: schema::SimulateRouteInput,
    ) -> Result<schema::SimulateRouteResult, CratestackError> {
        require_permission(ctx, "route:read")?;

        let sys = Self::sys();

        // Same validation `sendMessage`'s own step 2 applies (§3.2) — a
        // simulated recipient should be held to the identical "is this a
        // real mobile MSISDN" bar real dispatch would apply, not a looser
        // one that would let the simulator answer for a number sendMessage
        // itself would already have rejected before routing ever ran.
        let msisdn = Msisdn::parse_mobile(&args.msisdn)
            .map_err(|error| CratestackError::Validation(error.to_string()))?;
        let operator = self
            .classify_operator(db.operator_prefix_rule(), &sys, &msisdn)
            .await?;

        let (routes, providers) = route_simulator::fetch_routes_and_providers(db, &sys).await?;
        let no_routes_configured = routes.is_empty();

        let candidate = sms_routing::RoutingCandidate {
            operator: route_simulator::convert_operator(operator),
            class: route_simulator::convert_class(args.class),
            app_id: &args.appId,
            msisdn_national: msisdn.national(),
        };

        // §54's own load-bearing property: injecting the draw, not
        // generating it internally, is what makes this procedure a genuine
        // replay of `sms_routing::select_route` rather than a second
        // decision engine that happens to agree with production most of
        // the time. `args.draw` lets a caller pin an exact value to see how
        // a specific draw resolves a tie; omitting it draws a fresh,
        // realistic sample the same way `backends/crates/sms-worker/src/routing.rs::decide`
        // does for a real dispatch.
        let draw = args.draw.unwrap_or_else(rand::random);

        let decision = sms_routing::select_route(
            &routes,
            &providers,
            &candidate,
            &sms_routing::ExcludedRouteIds::new(),
            draw,
        );

        Ok(route_simulator::decision_to_wire(
            &decision,
            operator,
            msisdn.national(),
            no_routes_configured,
        ))
    }

    /// #50: the message detail timeline's own data source.
    ///
    /// `DeliveryReceipt`'s own `list`/`detail` policy is
    /// `auth().kind == "user" || hasRole('system')` — it has never admitted
    /// `kind == "app"`, the only real caller kind this deployment mints
    /// (`SignedAuth::context_for` hardcodes `role: "app"` for every
    /// app key; see `auth.rs`'s own doc). So the console's own
    /// credential cannot read `GET /delivery_receipts` directly, the same
    /// structural wall #59's own finding already named for `Provider`/
    /// `WebhookEndpoint`. `@authorize(Message, detail, args.messageId)` on
    /// the schema declaration is what closes that gap *safely*: it runs
    /// before this function body ever executes, using the caller's own
    /// real `ctx` — not `sys()` — against `Message`'s own `detail` policy
    /// (`appId == auth().appId || hasRole('system')`), so a caller whose
    /// token doesn't already own the referenced message never reaches this
    /// line at all. Only once that's confirmed does this function read
    /// under `sys()`, the same "declaratively gate entry, then read
    /// broadly inside" shape `cancelMessage`/`replayWebhookAttempt` already
    /// establish elsewhere in this file.
    ///
    /// `require_permission(ctx, "sms:read")` is Layer 2, on top of Layer
    /// 1's broad `@allow(auth().kind == "app" || ...)` — the same
    /// two-layer shape `worker_lock_snapshot` above uses for `worker:read`.
    /// `sms:read` has been in `docs/architecture.md` §5.2's own scope
    /// vocabulary and in `deploy/.env.example`'s `SMS_CONSOLE_SCOPE` since
    /// #22/#24, provisioned but never actually checked anywhere — this is
    /// its first real consumer.
    ///
    /// Ordered oldest-first (`receivedAt` ascending): the console's own
    /// timeline reads top-to-bottom as "what happened, in order," and
    /// `DeliveryReceipt.receivedAt` (when this system persisted the row,
    /// not `occurredAt`, the provider's own optional, less trustworthy
    /// timestamp) is the one ordering key every row is guaranteed to have.
    async fn message_receipts(
        &self,
        db: &schema::Cratestack,
        ctx: &CratestackContext,
        args: schema::MessageReceiptsInput,
    ) -> Result<schema::MessageReceiptsResult, CratestackError> {
        require_permission(ctx, "sms:read")?;

        let sys = Self::sys();
        let rows = db
            .delivery_receipt()
            .find_many()
            .where_expr(FilterExpr::from(
                delivery_receipt::messageId().eq(args.messageId),
            ))
            .order_by(delivery_receipt::receivedAt().asc())
            .run(&sys)
            .await?;

        let receipts = rows
            .into_iter()
            .map(|row| schema::DeliveryReceiptSummary {
                id: row.id,
                providerId: row.providerId,
                outcome: row.outcome,
                rawStatus: row.rawStatus,
                errorCode: row.errorCode,
                networkCode: row.networkCode,
                receivedAt: row.receivedAt,
                occurredAt: row.occurredAt,
            })
            .collect();

        Ok(schema::MessageReceiptsResult { receipts })
    }

    /// `kind:role:appId`, read straight off the same `Principal` fields
    /// `into_context` writes (`auth.rs`). Two different callers must never
    /// share a cache entry when they'd see different rows: a `kind ==
    /// "app"` caller's `Message` reads are scoped to its own `appId`
    /// (Message's own `@@allow`), while a `kind == "user"` caller's are
    /// not (see `dashboard_snapshot`'s own doc). Role is included even
    /// though it doesn't currently change which rows are visible, because
    /// `require_permission` denies before this key is ever built — a role
    /// change that flips a caller from denied to allowed must not read a
    /// stale allowed-shaped cache entry belonging to a different role that
    /// happened to share `kind`/`appId`.
    fn dashboard_cache_key(ctx: &CratestackContext) -> String {
        // `kind`/`role`/`appId` are `auth()`-queryable fields
        // (`Principal::into_context`, `auth.rs`) — a different bag from
        // `perms`/`scope`, which `require_permission` reads out of
        // `ctx.extensions` instead. Read via `auth_field`, not
        // `ctx.extensions.get`: the latter is always empty for these three
        // (confirmed live — see `dashboard_snapshot`'s own `appId` doc).
        let field = |name: &str| match ctx.auth_field(name) {
            Some(Value::String(s)) => s.clone(),
            _ => String::new(),
        };
        format!("{}:{}:{}", field("kind"), field("role"), field("appId"))
    }

    /// #49: the operator dashboard's one data call. Gated on
    /// `require_permission(ctx, "dashboard:read")` before anything else
    /// runs — the same shape `worker_lock_snapshot`/`simulate` already
    /// use, and for the identical reason: `DashboardSummary`'s own
    /// `@allow` admits any `auth().kind == "app"` caller unconditionally
    /// (no row to scope a predicate by — it isn't a model), so a granted
    /// `dashboard:read` scope is the real perimeter for a provisioned app
    /// client today, not defense in depth.
    ///
    /// Every field below is read under the caller's own `ctx`, never
    /// `Self::sys()` — deliberately, so the numbers this returns are
    /// exactly what that caller's own token can see, honouring each
    /// model's row policy rather than bypassing it:
    /// - `Message` (`queueDepth`, `stuckMessages`, `operatorStats`,
    ///   `hourlyBuckets`): `appId == auth().appId` for an app-kind caller
    ///   (this console's own machine credential, today — see #211) or
    ///   unscoped for a human (`auth().kind == "user"`). The console's own
    ///   scope banner precedent (`messages-screen.tsx`) is why the wire
    ///   result carries `appId` back — so the UI can say what it's
    ///   scoped to rather than leave an operator to guess from an
    ///   unfamiliar row count.
    /// - `Job` (`jobBacklog`): unscoped for any `auth().kind == "app"`
    ///   caller (`Job` carries no `appId` at all) — system-wide, matching
    ///   the Jobs screen's own banner.
    /// - `WebhookAttempt` (`outboxDepth`): scoped to `endpoint.appId ==
    ///   auth().appId` — `WebhookAttempt`'s `@@allow` has no unscoped
    ///   `auth().kind == "app"` clause, unlike `Job`.
    ///
    /// No `GROUP BY` exists in `cratestack-sqlx =0.7.10`'s `aggregate()`
    /// (`count`/`sum`/`avg`/`min`/`max` only, each a single filtered scalar
    /// — see `schema.cstack`'s own comment on `DashboardSummary`), so every
    /// bucket/operator/state combination below is its own `aggregate()
    /// .count()` call rather than one grouped query: 6 hourly buckets × 2
    /// (total, UCS-2) + 5 operators × 2 (delivered, terminal) +
    /// queueDepth + jobBacklog + outboxDepth + stuckMessages = 26 small,
    /// indexed `COUNT` queries per snapshot. Accepted rather than reached
    /// for a raw-SQL R1 exception: unlike every existing exception (see
    /// `CONTRIBUTING.md`'s table), this *is* a real schema-backed model
    /// read — a raw query would have to hand-reimplement row policy,
    /// `@@retain` purge semantics, and soft-delete scoping itself, rather
    /// than getting all three for free the way `aggregate()` does (its own
    /// doc comment: "filtered through the model's read policy AND the
    /// soft-delete column"). `dashboard_cache`'s 15s TTL is what keeps 26
    /// queries per *load* from becoming 26 queries per *poll interval*.
    /// Six rolling hours, oldest first. `[5]` is the current, in-progress
    /// hour. Throughput and the UCS-2-ratio trend both read off this one
    /// array — see `HourlyBucket`'s own schema comment for why that's
    /// deliberate, not an accident of reuse. Split out of
    /// `dashboard_snapshot` purely to stay under clippy's line-count lint;
    /// no reuse beyond that one caller.
    async fn dashboard_hourly_buckets(
        db: &schema::Cratestack,
        ctx: &CratestackContext,
        now: DateTime<Utc>,
    ) -> Result<Vec<schema::HourlyBucket>, CratestackError> {
        let mut hourly_buckets = Vec::with_capacity(6);
        for hours_ago in (0..6i64).rev() {
            let bucket_end = now - ChronoDuration::hours(hours_ago);
            let bucket_start = bucket_end - ChronoDuration::hours(1);
            let window = FilterExpr::from(message::createdAt().gte(bucket_start))
                .and(message::createdAt().lt(bucket_end));

            let total_count = db
                .message()
                .aggregate()
                .count()
                .where_expr(window.clone())
                .run(ctx)
                .await?;
            let ucs2_count = db
                .message()
                .aggregate()
                .count()
                .where_expr(window.and(message::encoding().eq(schema::Encoding::ucs2)))
                .run(ctx)
                .await?;

            hourly_buckets.push(schema::HourlyBucket {
                bucketStart: bucket_start,
                totalCount: total_count,
                ucs2Count: ucs2_count,
            });
        }
        Ok(hourly_buckets)
    }

    /// Trailing 24h, one row per `OperatorCode` variant. `terminalTotal`
    /// excludes `uncertain`/`undelivered` on purpose — see
    /// `OperatorDeliveryStats`'s own schema comment, and
    /// `dashboard_summary_live_postgres.rs`'s own headline test for the
    /// live proof that getting this wrong is caught, not just asserted.
    async fn dashboard_operator_stats(
        db: &schema::Cratestack,
        ctx: &CratestackContext,
        now: DateTime<Utc>,
    ) -> Result<Vec<schema::OperatorDeliveryStats>, CratestackError> {
        let window_start = now - ChronoDuration::hours(24);
        let mut operator_stats = Vec::with_capacity(5);
        for operator in [
            schema::OperatorCode::mtn,
            schema::OperatorCode::orange,
            schema::OperatorCode::camtel,
            schema::OperatorCode::nexttel,
            schema::OperatorCode::unknown,
        ] {
            let in_window = FilterExpr::from(message::operator().eq(operator))
                .and(message::createdAt().gte(window_start));

            let delivered = db
                .message()
                .aggregate()
                .count()
                .where_expr(
                    in_window
                        .clone()
                        .and(message::state().eq(schema::MessageState::delivered)),
                )
                .run(ctx)
                .await?;
            let terminal_total = db
                .message()
                .aggregate()
                .count()
                .where_expr(in_window.and(message::state().in_([
                    schema::MessageState::delivered,
                    schema::MessageState::failed,
                    schema::MessageState::expired,
                    schema::MessageState::rejected,
                    schema::MessageState::cancelled,
                ])))
                .run(ctx)
                .await?;

            operator_stats.push(schema::OperatorDeliveryStats {
                operator,
                delivered,
                terminalTotal: terminal_total,
            });
        }
        Ok(operator_stats)
    }

    /// The four live, not-time-windowed gauges — `queueDepth`,
    /// `stuckMessages`, `jobBacklog`, `outboxDepth` — in that order.
    async fn dashboard_live_gauges(
        db: &schema::Cratestack,
        ctx: &CratestackContext,
    ) -> Result<(i64, i64, i64, i64), CratestackError> {
        let queue_depth = db
            .message()
            .aggregate()
            .count()
            .where_expr(FilterExpr::from(message::state().in_([
                schema::MessageState::accepted,
                schema::MessageState::queued,
                schema::MessageState::routed,
            ])))
            .run(ctx)
            .await?;

        let stuck_messages = db
            .message()
            .aggregate()
            .count()
            .where_expr(FilterExpr::from(message::state().in_([
                schema::MessageState::uncertain,
                schema::MessageState::undelivered,
            ])))
            .run(ctx)
            .await?;

        let job_backlog = db
            .job()
            .aggregate()
            .count()
            .where_expr(FilterExpr::from(
                job::state().in_([schema::JobState::pending, schema::JobState::running]),
            ))
            .run(ctx)
            .await?;

        let outbox_depth = db
            .webhook_attempt()
            .aggregate()
            .count()
            .where_expr(FilterExpr::from(webhook_attempt::state().in_([
                schema::AttemptState::pending,
                schema::AttemptState::delivering,
            ])))
            .run(ctx)
            .await?;

        Ok((queue_depth, stuck_messages, job_backlog, outbox_depth))
    }

    async fn dashboard_snapshot(
        &self,
        db: &schema::Cratestack,
        ctx: &CratestackContext,
    ) -> Result<schema::DashboardSummary, CratestackError> {
        require_permission(ctx, "dashboard:read")?;

        let key = Self::dashboard_cache_key(ctx);
        self.dashboard_cache
            .get_or_fetch(key, |_key| async move {
                let now = Utc::now();

                let hourly_buckets = Self::dashboard_hourly_buckets(db, ctx, now).await?;
                let operator_stats = Self::dashboard_operator_stats(db, ctx, now).await?;
                let (queue_depth, stuck_messages, job_backlog, outbox_depth) =
                    Self::dashboard_live_gauges(db, ctx).await?;

                // Found live, writing this file's own test: `appId` is an
                // `auth()`-queryable field (`Principal::into_context`),
                // not an extension — `ctx.extensions.get("appId")` is
                // always `None` here, silently. `auth_field` is the real
                // accessor; `require_permission`'s own `perms`/`scope`
                // genuinely do live in `ctx.extensions`, which is what
                // made this easy to get wrong by analogy.
                let app_id = match ctx.auth_field("appId") {
                    Some(Value::String(s)) if !s.is_empty() => Some(s.clone()),
                    _ => None,
                };

                Ok::<_, CratestackError>(schema::DashboardSummary {
                    generatedAt: now,
                    appId: app_id,
                    queueDepth: queue_depth,
                    jobBacklog: job_backlog,
                    outboxDepth: outbox_depth,
                    stuckMessages: stuck_messages,
                    operatorStats: operator_stats,
                    hourlyBuckets: hourly_buckets,
                })
            })
            .await
    }

    /// #52/#58: `provisionUser` — see `schema.cstack`'s own doc on this
    /// procedure for the reasoning (why it is a procedure rather than a plain
    /// `POST /users`, and why there is no retire counterpart). Since VS-4 it
    /// makes a `User` row and no credential: no password, token or recovery
    /// code exists to generate, so nothing is returned once. Two writes in
    /// one transaction: `User.subject` is `@unique` and this delegate
    /// doesn't know its own generated id ahead of the `create()` call, so a
    /// unique placeholder is written first and corrected to the row's own id
    /// in a second update — the identical shape `sms-gateway provision-user`
    /// and console pairing's just-in-time user use.
    ///
    /// Both writes run under the *caller's own* `ctx`: `User.create`/
    /// `User.update`'s own `@@allow` admits `hasRole('owner') ||
    /// hasRole('admin')` — no `hasRole('system')` — and this procedure's own
    /// `@allow` is exactly as restrictive.
    async fn provision_console_user(
        &self,
        db: &schema::IsolatedCratestack,
        ctx: &CratestackContext,
        args: schema::ProvisionUserInput,
    ) -> Result<schema::ProvisionUserResult, CratestackError> {
        require_permission(ctx, "user:manage")?;

        let email = args.email;
        let display_name = args.displayName;
        let role_key = args.roleKey;

        let user_id = db
            .transaction(async |tx| {
                let user = db
                    .user()
                    .create(schema::CreateUserInput {
                        subject: format!("pending-{}", cratestack::uuid::Uuid::new_v4()),
                        email: email.clone(),
                        displayName: display_name,
                        roleKey: role_key.clone(),
                        lastLoginAt: None,
                        deletedAt: None,
                    })
                    .run_in_tx(tx, ctx)
                    .await?
                    // cratestack 0.7.13 (#554): see the identical
                    // comment on `provisionAppClient`'s own
                    // `run_in_tx` call above.
                    .value;
                let user_id = user.id.clone();

                db.user()
                    .update(user.id.clone())
                    .set(schema::UpdateUserInput {
                        subject: Some(user.id.clone()),
                        ..Default::default()
                    })
                    .if_match(user.version)
                    .run_in_tx(tx, ctx)
                    .await?;

                Ok(user_id)
            })
            .await
            .map_err(map_database_error)?;

        Ok(schema::ProvisionUserResult {
            userId: user_id,
            email,
            roleKey: role_key,
        })
    }

    /// #58: hash `args.msisdn` under this crate's own pepper and record an
    /// `OptOut` — see `schema.cstack`'s own doc on `recordOptOut` for why
    /// this needs to be a procedure (the console has no access to
    /// `SMS_HASH_PEPPER`) and why it admits `hasRole('support')` at the
    /// procedure level even though `OptOut.create`'s own `@@allow` does
    /// not. Runs under `sys` unconditionally — the same reason, not a new
    /// one: it is what lets a `support`-role caller's write succeed at all
    /// without touching `OptOut.create`'s own policy.
    async fn create_opt_out_entry(
        &self,
        db: &schema::IsolatedCratestack,
        ctx: &CratestackContext,
        args: schema::RecordOptOutInput,
    ) -> Result<schema::OptOut, CratestackError> {
        require_permission(ctx, "optout:manage")?;

        let msisdn = Msisdn::parse_mobile(&args.msisdn)
            .map_err(|error| CratestackError::Validation(error.to_string()))?;
        let msisdn_hash = self.keyed_hash_hex(msisdn.as_e164());

        let sys = Self::sys();
        db.opt_out()
            .create(schema::CreateOptOutInput {
                msisdnHash: msisdn_hash,
                msisdn: msisdn.as_e164().to_owned(),
                source: args.source,
                scope: args.scope,
                reason: args.reason,
                optedOutAt: Utc::now(),
            })
            .run(&sys)
            .await
            .map_err(map_database_error)
    }

    /// #58: `searchOptOutByMsisdn` — hash `args.msisdn` under this crate's
    /// own pepper and look up the one `OptOut` row (`msisdnHash` is
    /// `@unique`) it could ever match, if any. See `schema.cstack`'s own
    /// doc on `searchOptOutByMsisdn` for what "not found" can honestly
    /// mean here.
    ///
    /// **The house standard this ticket asks for, said in code, not just
    /// in the doc comment above it: this can never fall back to an
    /// unfiltered list.** `where_expr` always carries the equality
    /// predicate below — there is no code path in this function that omits
    /// it, so a hash that matches nothing produces an empty `Vec`, never
    /// "everything."
    ///
    /// Broken deliberately once, for real, to prove the live test actually
    /// guards this rather than passing by construction:
    /// `backends/crates/sms-api/tests/console_identity_live_postgres.rs`'s own
    /// `searching_an_unrecorded_number_finds_nothing_even_with_other_rows_present`
    /// seeds three unrelated `OptOut` rows, then searches for a fourth
    /// number that was never recorded. With `.where_expr(...)` below
    /// temporarily commented out (an unfiltered `find_many().limit(1)`),
    /// the test failed with:
    ///
    /// ```text
    /// a search that matches nothing must return None, not one of the
    /// unrelated seeded rows: got Some(OptOutSummary { id:
    /// "c13e89f454a21777db190cb", msisdnHash: "hmac-sha256-v1:0b0b17...",
    /// source: admin, scope: "all", reason: None, optedOutAt:
    /// 2026-08-12T23:35:06.486625Z, createdAt:
    /// 2026-08-12T23:35:06.486733Z })
    /// ```
    ///
    /// — a real, unrelated opt-out row, returned as though it matched.
    /// Restoring the `where_expr` call restored a pass on all seven tests
    /// in that file.
    async fn search_opt_out(
        &self,
        db: &schema::Cratestack,
        ctx: &CratestackContext,
        args: schema::OptOutSearchInput,
    ) -> Result<schema::OptOutSearchResult, CratestackError> {
        require_permission(ctx, "optout:manage")?;

        let msisdn = Msisdn::parse_mobile(&args.msisdn)
            .map_err(|error| CratestackError::Validation(error.to_string()))?;
        let msisdn_hash = self.keyed_hash_hex(msisdn.as_e164());

        let sys = Self::sys();
        let matches = db
            .opt_out()
            .find_many()
            .where_expr(FilterExpr::from(opt_out::msisdnHash().eq(msisdn_hash)))
            .limit(1)
            .run(&sys)
            .await?;

        Ok(schema::OptOutSearchResult {
            optOut: matches.into_iter().next().map(to_opt_out_summary),
        })
    }

    /// #58: `auditLog` — a filtered, paged window over `cratestack_audit`.
    /// See `crate::audit_log`'s own module doc for the mechanism; this is
    /// a thin translation between the wire query/page shape and that
    /// module's own filter/page types.
    async fn list_audit_log(
        &self,
        db: &schema::Cratestack,
        ctx: &CratestackContext,
        args: schema::AuditLogQuery,
    ) -> Result<schema::AuditLogPage, CratestackError> {
        require_permission(ctx, "audit:read")?;

        let filter = audit_log::AuditLogFilter {
            model: args.model,
            operation: args.operation,
            actor_id: args.actorId,
            since: args.since,
            until: args.until,
        };
        let limit = args.limit.unwrap_or(50);
        let offset = args.offset.unwrap_or(0);
        let page = audit_log::list_audit_entries(db, &filter, limit, offset).await?;

        Ok(schema::AuditLogPage {
            entries: page.entries.into_iter().map(to_audit_log_entry).collect(),
            hasMore: page.has_more,
        })
    }

    /// #58: `auditChainStatus` — see `crate::audit_log::chain_status`'s own
    /// doc. Read-only, computed live every call (cheap: see that module's
    /// own reasoning), never writes an `AuditAnchor` row.
    async fn audit_chain_snapshot(
        &self,
        db: &schema::Cratestack,
        ctx: &CratestackContext,
    ) -> Result<schema::AuditChainStatus, CratestackError> {
        require_permission(ctx, "audit:read")?;

        let sys = Self::sys();
        let status = audit_log::chain_status(db, &sys).await?;

        Ok(schema::AuditChainStatus {
            latestAnchorId: status.latest_anchor.as_ref().map(|a| a.id.clone()),
            latestPeriodEnd: status.latest_anchor.as_ref().map(|a| a.periodEnd),
            latestRowCount: status.latest_anchor.as_ref().map(|a| a.rowCount),
            linkageBreaks: status.linkage_breaks,
            latestContentVerified: status.latest_period_content_verified,
        })
    }
}

/// [`schema::OptOut`] flattened onto the console's own bespoke wire type —
/// see `schema.cstack`'s own comment on `OptOutSummary` for why this isn't
/// the raw model.
/// What a switch of `endpoint` to `mode` writes, or why it is refused (`422`).
/// Pure, so the rules are tested without a database; the CHECKs of
/// `0010_sealed_webhooks` stay the backstop for every other writer.
fn plan_mode_switch(
    endpoint: &schema::WebhookEndpoint,
    mode: schema::WebhookDeliveryMode,
    audience: Option<&str>,
) -> Result<schema::UpdateWebhookEndpointInput, CratestackError> {
    let refuse = |message: String| Err(CratestackError::Validation(message));
    if endpoint.deliveryMode == mode {
        return refuse(format!(
            "webhook endpoint {} is already {}: there is nothing to switch",
            endpoint.id,
            mode_name(mode)
        ));
    }
    match mode {
        schema::WebhookDeliveryMode::hmac => {
            if audience.is_some() {
                return refuse("an hmac endpoint has no audience: leave it out".to_owned());
            }
            Ok(schema::UpdateWebhookEndpointInput {
                deliveryMode: Some(schema::WebhookDeliveryMode::hmac),
                audience: Some(None),
                secret: Some(Some(sms_webhook::generate_secret())),
                prevSecret: Some(None),
                secretRotatedAt: Some(None),
                ..Default::default()
            })
        }
        schema::WebhookDeliveryMode::sealed => {
            let Some(audience) = audience.filter(|audience| !audience.is_empty()) else {
                return refuse(
                    "a sealed endpoint needs an audience: the name its receiver answers to"
                        .to_owned(),
                );
            };
            if !sms_webhook::url_can_be_sealed(&endpoint.url) {
                // The URL is not echoed: it may carry credentials.
                let reason = match sms_webhook::delivery_route(&endpoint.url) {
                    Err(sms_webhook::SealError::Url(reason)) => reason,
                    _ => "its host or port is not written in the form a sealed endpoint takes",
                };
                return refuse(format!(
                    "webhook endpoint {}'s URL cannot carry a sealed delivery ({reason}): \
                     change the URL first",
                    endpoint.id
                ));
            }
            Ok(schema::UpdateWebhookEndpointInput {
                deliveryMode: Some(schema::WebhookDeliveryMode::sealed),
                audience: Some(Some(audience.to_owned())),
                secret: Some(None),
                prevSecret: Some(None),
                secretRotatedAt: Some(None),
                ..Default::default()
            })
        }
    }
}

const fn mode_name(mode: schema::WebhookDeliveryMode) -> &'static str {
    match mode {
        schema::WebhookDeliveryMode::sealed => "sealed",
        schema::WebhookDeliveryMode::hmac => "hmac",
    }
}

fn to_opt_out_summary(row: schema::OptOut) -> schema::OptOutSummary {
    schema::OptOutSummary {
        id: row.id,
        msisdnHash: row.msisdnHash,
        source: row.source,
        scope: row.scope,
        reason: row.reason,
        optedOutAt: row.optedOutAt,
        createdAt: row.createdAt,
    }
}

/// [`audit_log::AuditLogEntry`] onto the wire — the crate-internal
/// `snake_case` struct `crate::audit_log` returns, camelCased for the
/// generated `AuditLogEntry` type.
fn to_audit_log_entry(entry: audit_log::AuditLogEntry) -> schema::AuditLogEntry {
    schema::AuditLogEntry {
        eventId: entry.event_id,
        model: entry.model,
        operation: entry.operation,
        primaryKey: entry.primary_key,
        actor: entry.actor,
        tenant: entry.tenant,
        before: entry.before,
        after: entry.after,
        requestId: entry.request_id,
        occurredAt: entry.occurred_at,
    }
}

impl schema::procedures::ProcedureRegistry for Procedures {
    fn preview_message(
        &self,
        _db: &schema::Cratestack,
        _ctx: &CratestackContext,
        args: schema::procedures::preview_message::Args,
        // cratestack 0.7.13 (cratestack#512): every `ProcedureRegistry`
        // method gained a trailing, unconstructible witness parameter —
        // proof this call went through `authorize_with_db`/`invoke_with_db`
        // rather than skipping `@allow`. Never read; see procedures.rs's
        // own module doc if this pattern needs re-deriving.
        _authorized: schema::procedures::preview_message::Authorized,
    ) -> impl core::future::Future<
        Output = Result<schema::procedures::preview_message::Output, CratestackError>,
    > + Send {
        core::future::ready(Self::preview(&args.args))
    }

    fn send_message(
        &self,
        db: &schema::IsolatedCratestack,
        ctx: &CratestackContext,
        args: schema::procedures::send_message::Args,
        // cratestack 0.7.13 (cratestack#512): see `preview_message`'s
        // identical comment above.
        _authorized: schema::procedures::send_message::Authorized,
    ) -> impl core::future::Future<
        Output = Result<schema::procedures::send_message::Output, CratestackError>,
    > + Send {
        self.send(db, ctx, args.args)
    }

    fn list_messages_page(
        &self,
        _db: &schema::Cratestack,
        _ctx: &CratestackContext,
        _args: schema::procedures::list_messages_page::Args,
        // cratestack 0.7.13 (cratestack#512): see `preview_message`'s
        // identical comment above.
        _authorized: schema::procedures::list_messages_page::Authorized,
    ) -> impl core::future::Future<
        Output = Result<schema::procedures::list_messages_page::Output, CratestackError>,
    > + Send {
        core::future::ready(Err(not_yet("listMessagesPage", "milestone 2")))
    }

    fn cancel_message(
        &self,
        _db: &schema::IsolatedCratestack,
        _ctx: &CratestackContext,
        _args: schema::procedures::cancel_message::Args,
        // cratestack 0.7.13 (cratestack#512): see `preview_message`'s
        // identical comment above.
        _authorized: schema::procedures::cancel_message::Authorized,
    ) -> impl core::future::Future<
        Output = Result<schema::procedures::cancel_message::Output, CratestackError>,
    > + Send {
        core::future::ready(Err(not_yet("cancelMessage", "milestone 2")))
    }

    fn enqueue_job(
        &self,
        _db: &schema::IsolatedCratestack,
        _ctx: &CratestackContext,
        _args: schema::procedures::enqueue_job::Args,
        // cratestack 0.7.13 (cratestack#512): see `preview_message`'s
        // identical comment above.
        _authorized: schema::procedures::enqueue_job::Authorized,
    ) -> impl core::future::Future<
        Output = Result<schema::procedures::enqueue_job::Output, CratestackError>,
    > + Send {
        core::future::ready(Err(not_yet("enqueueJob", "milestone 2 (the jobs role)")))
    }

    fn provision_app_client(
        &self,
        db: &schema::IsolatedCratestack,
        ctx: &CratestackContext,
        args: schema::procedures::provision_app_client::Args,
        // cratestack 0.7.13 (cratestack#512): see `preview_message`'s
        // identical comment above.
        _authorized: schema::procedures::provision_app_client::Authorized,
    ) -> impl core::future::Future<
        Output = Result<schema::procedures::provision_app_client::Output, CratestackError>,
    > + Send {
        self.provision_client(db, ctx, args.args)
    }

    fn register_app_client_key(
        &self,
        db: &schema::IsolatedCratestack,
        _ctx: &CratestackContext,
        args: schema::procedures::register_app_client_key::Args,
        // cratestack 0.7.13 (cratestack#512): see `preview_message`'s
        // identical comment above.
        _authorized: schema::procedures::register_app_client_key::Authorized,
    ) -> impl core::future::Future<
        Output = Result<schema::procedures::register_app_client_key::Output, CratestackError>,
    > + Send {
        let sys = Self::sys();
        async move {
            crate::client_keys::register(
                db,
                &sys,
                args.args,
                crate::client_keys::Registrant::Caller,
            )
            .await
        }
    }

    fn revoke_app_client_key(
        &self,
        db: &schema::IsolatedCratestack,
        _ctx: &CratestackContext,
        args: schema::procedures::revoke_app_client_key::Args,
        // cratestack 0.7.13 (cratestack#512): see `preview_message`'s
        // identical comment above.
        _authorized: schema::procedures::revoke_app_client_key::Authorized,
    ) -> impl core::future::Future<
        Output = Result<schema::procedures::revoke_app_client_key::Output, CratestackError>,
    > + Send {
        let sys = Self::sys();
        async move { crate::client_keys::revoke(db, &sys, args.args).await }
    }

    fn register_console_key(
        &self,
        db: &schema::IsolatedCratestack,
        ctx: &CratestackContext,
        args: schema::procedures::register_console_key::Args,
        // cratestack 0.7.13 (cratestack#512): see `preview_message`'s
        // identical comment above.
        _authorized: schema::procedures::register_console_key::Authorized,
    ) -> impl core::future::Future<
        Output = Result<schema::procedures::register_console_key::Output, CratestackError>,
    > + Send {
        let sys = Self::sys();
        // Layer 2: `@allow(auth().kind == "app")` cannot say which app. The
        // scope is checked here, before any work, so a refused caller costs
        // no signature verification and no query.
        let scope = crate::console_keys::require_pair_scope(ctx);
        async move {
            scope?;
            crate::console_keys::register(db, &sys, args.args, chrono::Utc::now()).await
        }
    }

    fn revoke_console_key(
        &self,
        db: &schema::IsolatedCratestack,
        _ctx: &CratestackContext,
        args: schema::procedures::revoke_console_key::Args,
        // cratestack 0.7.13 (cratestack#512): see `preview_message`'s
        // identical comment above.
        _authorized: schema::procedures::revoke_console_key::Authorized,
    ) -> impl core::future::Future<
        Output = Result<schema::procedures::revoke_console_key::Output, CratestackError>,
    > + Send {
        let sys = Self::sys();
        async move { crate::console_keys::revoke(db, &sys, args.args).await }
    }

    fn authorize_composition(
        &self,
        _db: &schema::Cratestack,
        ctx: &CratestackContext,
        args: schema::procedures::authorize_composition::Args,
        // cratestack 0.7.13 (cratestack#512): see `preview_message`'s
        // identical comment above.
        _authorized: schema::procedures::authorize_composition::Authorized,
    ) -> impl core::future::Future<
        Output = Result<schema::procedures::authorize_composition::Output, CratestackError>,
    > + Send {
        core::future::ready(crate::composition::authorize(ctx, args.args))
    }

    fn rotate_webhook_secret(
        &self,
        db: &schema::IsolatedCratestack,
        ctx: &CratestackContext,
        args: schema::procedures::rotate_webhook_secret::Args,
        // cratestack 0.7.13 (cratestack#512): see `preview_message`'s
        // identical comment above.
        _authorized: schema::procedures::rotate_webhook_secret::Authorized,
    ) -> impl core::future::Future<
        Output = Result<schema::procedures::rotate_webhook_secret::Output, CratestackError>,
    > + Send {
        self.rotate_secret(db, ctx, args.args)
    }

    fn switch_webhook_delivery_mode(
        &self,
        db: &schema::IsolatedCratestack,
        ctx: &CratestackContext,
        args: schema::procedures::switch_webhook_delivery_mode::Args,
        // cratestack 0.7.13 (cratestack#512): see `preview_message`'s
        // identical comment above.
        _authorized: schema::procedures::switch_webhook_delivery_mode::Authorized,
    ) -> impl core::future::Future<
        Output = Result<schema::procedures::switch_webhook_delivery_mode::Output, CratestackError>,
    > + Send {
        self.switch_mode(db, ctx, args.args)
    }

    fn replay_webhook_attempt(
        &self,
        db: &schema::IsolatedCratestack,
        ctx: &CratestackContext,
        args: schema::procedures::replay_webhook_attempt::Args,
        // cratestack 0.7.13 (cratestack#512): see `preview_message`'s
        // identical comment above.
        _authorized: schema::procedures::replay_webhook_attempt::Authorized,
    ) -> impl core::future::Future<
        Output = Result<schema::procedures::replay_webhook_attempt::Output, CratestackError>,
    > + Send {
        self.replay_attempt(db, ctx, args.args)
    }

    fn requeue_job(
        &self,
        db: &schema::IsolatedCratestack,
        ctx: &CratestackContext,
        args: schema::procedures::requeue_job::Args,
        // cratestack 0.7.13 (cratestack#512): see `preview_message`'s
        // identical comment above.
        _authorized: schema::procedures::requeue_job::Authorized,
    ) -> impl core::future::Future<
        Output = Result<schema::procedures::requeue_job::Output, CratestackError>,
    > + Send {
        self.requeue(db, ctx, args.args)
    }

    fn worker_locks(
        &self,
        db: &schema::Cratestack,
        ctx: &CratestackContext,
        _args: schema::procedures::worker_locks::Args,
        // cratestack 0.7.13 (cratestack#512): see `preview_message`'s
        // identical comment above.
        _authorized: schema::procedures::worker_locks::Authorized,
    ) -> impl core::future::Future<
        Output = Result<schema::procedures::worker_locks::Output, CratestackError>,
    > + Send {
        self.worker_lock_snapshot(db, ctx)
    }

    fn simulate_route(
        &self,
        db: &schema::Cratestack,
        ctx: &CratestackContext,
        args: schema::procedures::simulate_route::Args,
        // cratestack 0.7.13 (cratestack#512): see `preview_message`'s
        // identical comment above.
        _authorized: schema::procedures::simulate_route::Authorized,
    ) -> impl core::future::Future<
        Output = Result<schema::procedures::simulate_route::Output, CratestackError>,
    > + Send {
        self.simulate(db, ctx, args.args)
    }

    fn list_message_receipts(
        &self,
        db: &schema::Cratestack,
        ctx: &CratestackContext,
        args: schema::procedures::list_message_receipts::Args,
        // cratestack 0.7.13 (cratestack#512): see `preview_message`'s
        // identical comment above.
        _authorized: schema::procedures::list_message_receipts::Authorized,
    ) -> impl core::future::Future<
        Output = Result<schema::procedures::list_message_receipts::Output, CratestackError>,
    > + Send {
        self.message_receipts(db, ctx, args.args)
    }

    fn dashboard_summary(
        &self,
        db: &schema::Cratestack,
        ctx: &CratestackContext,
        _args: schema::procedures::dashboard_summary::Args,
        // cratestack 0.7.13 (cratestack#512): see `preview_message`'s
        // identical comment above.
        _authorized: schema::procedures::dashboard_summary::Authorized,
    ) -> impl core::future::Future<
        Output = Result<schema::procedures::dashboard_summary::Output, CratestackError>,
    > + Send {
        self.dashboard_snapshot(db, ctx)
    }

    fn provision_user(
        &self,
        db: &schema::IsolatedCratestack,
        ctx: &CratestackContext,
        args: schema::procedures::provision_user::Args,
        // cratestack 0.7.13 (cratestack#512): see `preview_message`'s
        // identical comment above.
        _authorized: schema::procedures::provision_user::Authorized,
    ) -> impl core::future::Future<
        Output = Result<schema::procedures::provision_user::Output, CratestackError>,
    > + Send {
        self.provision_console_user(db, ctx, args.args)
    }

    fn record_opt_out(
        &self,
        db: &schema::IsolatedCratestack,
        ctx: &CratestackContext,
        args: schema::procedures::record_opt_out::Args,
        // cratestack 0.7.13 (cratestack#512): see `preview_message`'s
        // identical comment above.
        _authorized: schema::procedures::record_opt_out::Authorized,
    ) -> impl core::future::Future<
        Output = Result<schema::procedures::record_opt_out::Output, CratestackError>,
    > + Send {
        self.create_opt_out_entry(db, ctx, args.args)
    }

    fn search_opt_out_by_msisdn(
        &self,
        db: &schema::Cratestack,
        ctx: &CratestackContext,
        args: schema::procedures::search_opt_out_by_msisdn::Args,
        // cratestack 0.7.13 (cratestack#512): see `preview_message`'s
        // identical comment above.
        _authorized: schema::procedures::search_opt_out_by_msisdn::Authorized,
    ) -> impl core::future::Future<
        Output = Result<schema::procedures::search_opt_out_by_msisdn::Output, CratestackError>,
    > + Send {
        self.search_opt_out(db, ctx, args.args)
    }

    fn audit_log(
        &self,
        db: &schema::Cratestack,
        ctx: &CratestackContext,
        args: schema::procedures::audit_log::Args,
        // cratestack 0.7.13 (cratestack#512): see `preview_message`'s
        // identical comment above.
        _authorized: schema::procedures::audit_log::Authorized,
    ) -> impl core::future::Future<
        Output = Result<schema::procedures::audit_log::Output, CratestackError>,
    > + Send {
        self.list_audit_log(db, ctx, args.args)
    }

    fn audit_chain_status(
        &self,
        db: &schema::Cratestack,
        ctx: &CratestackContext,
        _args: schema::procedures::audit_chain_status::Args,
        // cratestack 0.7.13 (cratestack#512): see `preview_message`'s
        // identical comment above.
        _authorized: schema::procedures::audit_chain_status::Authorized,
    ) -> impl core::future::Future<
        Output = Result<schema::procedures::audit_chain_status::Output, CratestackError>,
    > + Send {
        self.audit_chain_snapshot(db, ctx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn endpoint(mode: schema::WebhookDeliveryMode, url: &str) -> schema::WebhookEndpoint {
        let hmac = mode == schema::WebhookDeliveryMode::hmac;
        schema::WebhookEndpoint {
            createdAt: Utc::now(),
            updatedAt: Utc::now(),
            id: "cendpoint0000000000000".to_owned(),
            appId: "capp00000000000000000".to_owned(),
            url: url.to_owned(),
            deliveryMode: mode,
            audience: (!hmac).then(|| "vaam".to_owned()),
            eventTypes: " message.delivered ".to_owned(),
            secret: hmac.then(|| "a-secret-that-is-at-least-32-chars-long".to_owned()),
            prevSecret: None,
            secretRotatedAt: None,
            maskRecipient: true,
            active: true,
            maxAttempts: 8,
            circuitOpenUntil: None,
            consecutiveFailures: 0,
            version: 3,
        }
    }

    fn refusal(result: Result<schema::UpdateWebhookEndpointInput, CratestackError>) -> String {
        match result {
            Err(CratestackError::Validation(message)) => message,
            other => panic!("expected a 422, got {other:?}"),
        }
    }

    #[test]
    fn a_switch_to_hmac_mints_a_secret_and_frees_the_audience() {
        let sealed = endpoint(
            schema::WebhookDeliveryMode::sealed,
            "https://h.example.test/webhooks/vsms",
        );
        let change = plan_mode_switch(&sealed, schema::WebhookDeliveryMode::hmac, None).unwrap();
        assert_eq!(change.deliveryMode, Some(schema::WebhookDeliveryMode::hmac));
        assert_eq!(change.audience, Some(None));
        let secret = change.secret.clone().flatten().expect("a minted secret");
        assert!(secret.starts_with("whsec_") && secret.len() == 70);
        assert_eq!(change.prevSecret, Some(None));
        assert_eq!(change.secretRotatedAt, Some(None));
        // Two switches mint two secrets.
        let again = plan_mode_switch(&sealed, schema::WebhookDeliveryMode::hmac, None).unwrap();
        assert_ne!(again.secret, change.secret);
    }

    #[test]
    fn a_switch_to_hmac_refuses_an_audience() {
        let sealed = endpoint(
            schema::WebhookDeliveryMode::sealed,
            "https://h.example.test/x",
        );
        let message = refusal(plan_mode_switch(
            &sealed,
            schema::WebhookDeliveryMode::hmac,
            Some("squatted"),
        ));
        assert!(message.contains("no audience"), "{message}");
    }

    #[test]
    fn a_switch_to_sealed_names_the_audience_and_clears_every_secret_column() {
        let hmac = endpoint(
            schema::WebhookDeliveryMode::hmac,
            "https://h.example.test/webhooks/vsms",
        );
        let change =
            plan_mode_switch(&hmac, schema::WebhookDeliveryMode::sealed, Some("vaam")).unwrap();
        assert_eq!(
            change.deliveryMode,
            Some(schema::WebhookDeliveryMode::sealed)
        );
        assert_eq!(change.audience, Some(Some("vaam".to_owned())));
        assert_eq!(change.secret, Some(None));
        assert_eq!(change.prevSecret, Some(None));
        assert_eq!(change.secretRotatedAt, Some(None));
    }

    #[test]
    fn a_switch_to_sealed_needs_an_audience() {
        let hmac = endpoint(
            schema::WebhookDeliveryMode::hmac,
            "https://h.example.test/x",
        );
        for audience in [None, Some("")] {
            let message = refusal(plan_mode_switch(
                &hmac,
                schema::WebhookDeliveryMode::sealed,
                audience,
            ));
            assert!(message.contains("needs an audience"), "{message}");
        }
    }

    #[test]
    fn a_switch_to_sealed_refuses_a_url_a_delivery_cannot_be_sealed_for_without_echoing_it() {
        for (url, reason) in [
            (
                "https://u:hunter2@h.example.test/hook",
                "it carries credentials",
            ),
            (
                "https://h.example.test/hook?token=hunter2",
                "it carries a query",
            ),
            // Sealable for `delivery_route`, but not written the way the CHECK takes it.
            ("https://exämple.test/hook", "not written in the form"),
        ] {
            let hmac = endpoint(schema::WebhookDeliveryMode::hmac, url);
            let message = refusal(plan_mode_switch(
                &hmac,
                schema::WebhookDeliveryMode::sealed,
                Some("vaam"),
            ));
            assert!(message.contains(reason), "{url}: {message}");
            assert!(!message.contains("hunter2"), "the URL leaked: {message}");
        }
    }

    #[test]
    fn a_switch_to_the_mode_it_is_in_is_refused() {
        for mode in [
            schema::WebhookDeliveryMode::sealed,
            schema::WebhookDeliveryMode::hmac,
        ] {
            let same = endpoint(mode, "https://h.example.test/x");
            let message = refusal(plan_mode_switch(&same, mode, Some("vaam")));
            assert!(message.contains("already"), "{message}");
        }
    }

    fn preview(body: &str, to: Option<&str>) -> Result<schema::PreviewResult, CratestackError> {
        Procedures::preview(&schema::PreviewInput {
            body: body.to_owned(),
            to: to.map(str::to_owned),
        })
    }

    #[test]
    fn plain_french_stays_gsm7_in_one_segment() {
        let result = preview("Votre code est 4821. Il expire dans 5 minutes.", None).unwrap();
        assert_eq!(result.encoding, schema::Encoding::gsm7);
        assert_eq!(result.segments, 1);
        assert_eq!(result.perSegment, 160);
        assert_eq!(result.offending.len(), 0);
    }

    #[test]
    fn preview_normalises_before_it_measures() {
        // The raw body is UCS-2 because of the typographic apostrophe. The send
        // path would normalise it away, so the preview must too — quoting UCS-2
        // here would overstate the bill on every message with a smart quote.
        let result = preview("Bienvenue sur l\u{2019}application", None).unwrap();
        assert_eq!(result.encoding, schema::Encoding::gsm7);
        assert_eq!(result.offending.len(), 0);
    }

    #[test]
    fn a_cedilla_survives_normalisation_and_is_reported() {
        let result = preview("Votre paiement a ete recu, merci. Reçu N.4821", None).unwrap();
        assert_eq!(result.encoding, schema::Encoding::ucs2);
        assert_eq!(result.perSegment, 70);
        assert_eq!(result.offending, vec!["ç".to_owned()]);
        assert!(result.suggestion.is_some());
    }

    #[test]
    fn repeated_offenders_are_reported_once() {
        let result = preview("reçu reçu reçu", None).unwrap();
        assert_eq!(result.offending, vec!["ç".to_owned()]);
    }

    #[test]
    fn a_recipient_is_normalised_to_e164() {
        let result = preview("bonjour", Some("6 77 12 34 56")).unwrap();
        assert_eq!(result.normalizedTo.as_deref(), Some("+237677123456"));
    }

    #[test]
    fn an_undeliverable_recipient_is_a_validation_error_not_a_silent_pass() {
        // A fixed line parses as a valid Cameroon number and cannot receive an
        // SMS. Failing here beats failing on a DLR three seconds later.
        let error = preview("bonjour", Some("+237222123456")).unwrap_err();
        assert!(matches!(error, CratestackError::Validation(_)));
    }

    #[test]
    fn no_recipient_means_no_normalised_recipient() {
        assert_eq!(preview("bonjour", None).unwrap().normalizedTo, None);
        assert_eq!(preview("bonjour", Some("  ")).unwrap().normalizedTo, None);
    }

    #[test]
    fn operator_code_round_trips_through_its_string_form() {
        for code in [
            schema::OperatorCode::mtn,
            schema::OperatorCode::orange,
            schema::OperatorCode::camtel,
            schema::OperatorCode::nexttel,
            schema::OperatorCode::unknown,
        ] {
            assert_eq!(parse_operator_code(operator_code_str(code)), Some(code));
        }
    }

    #[test]
    fn an_unrecognised_operator_string_is_not_guessed_at() {
        assert_eq!(parse_operator_code("mtn_typo"), None);
    }

    /// #134: `Procedures::keyed_hash_hex` is a thin wrapper over
    /// `pepper::hmac_sha256_hex` — that function's own unit tests
    /// (`pepper.rs`) already cover determinism, the stored-value prefix,
    /// and pepper-sensitivity in isolation. This just proves the wrapper
    /// actually threads `self.pepper` through rather than, say, silently
    /// falling back to some other value.
    #[test]
    fn keyed_hash_hex_uses_this_procedures_own_pepper() {
        let pepper_a = HashPepper::new("a".repeat(crate::pepper::MIN_PEPPER_BYTES)).unwrap();
        let pepper_b = HashPepper::new("b".repeat(crate::pepper::MIN_PEPPER_BYTES)).unwrap();
        let procedures_a = Procedures::new(pepper_a.clone());
        let procedures_b = Procedures::new(pepper_b);

        let hash = procedures_a.keyed_hash_hex("+237677123456");
        assert!(hash.starts_with("hmac-sha256-v1:"));
        assert_eq!(
            hash,
            procedures_a.keyed_hash_hex("+237677123456"),
            "must be deterministic under the same pepper"
        );
        assert_ne!(
            hash,
            procedures_b.keyed_hash_hex("+237677123456"),
            "a different Procedures instance's pepper must produce a different hash"
        );
        assert_eq!(
            hash,
            hmac_sha256_hex(&pepper_a, "+237677123456"),
            "must match the free function directly, not some independent computation"
        );
    }

    #[test]
    fn month_start_is_midnight_on_the_first() {
        let now = chrono::Utc
            .with_ymd_and_hms(2026, 3, 17, 14, 32, 9)
            .unwrap();
        let start = month_start(now);
        assert_eq!(start.day(), 1);
        assert_eq!(start.hour(), 0);
        assert_eq!(start.minute(), 0);
        assert_eq!(start.second(), 0);
        assert_eq!(start.month(), 3);
        assert_eq!(start.year(), 2026);
    }

    #[test]
    fn otp_gets_the_short_default_validity() {
        assert_eq!(
            default_validity(schema::MessageClass::otp),
            ChronoDuration::minutes(15)
        );
    }

    #[test]
    fn non_otp_classes_get_the_long_default_validity() {
        for class in [
            schema::MessageClass::transactional,
            schema::MessageClass::notification,
            schema::MessageClass::marketing,
        ] {
            assert_eq!(
                default_validity(class),
                ChronoDuration::hours(24),
                "{class:?}"
            );
        }
    }
}
