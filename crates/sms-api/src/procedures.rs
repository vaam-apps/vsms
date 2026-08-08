//! The seven procedures the schema declares.
//!
//! `previewMessage`, `sendMessage`, and `provisionAppClient` are
//! implemented. The other four touch the job queue or webhooks, none of
//! which exist yet; each returns a clearly-labelled error naming the
//! milestone that will build it, rather than a plausible-looking stub that
//! would pass a smoke test and lie.

use authkestra_engine::TokenManager;
use chrono::{DateTime, Datelike, Duration as ChronoDuration, Timelike, Utc};
use cratestack::{
    run_in_isolated_tx, CoolContext, CoolError, Decimal, FilterExpr, TransactionIsolation, Value,
};
use rand::rngs::OsRng;
use rsa::pkcs8::{EncodePrivateKey, LineEnding};
use rsa::RsaPrivateKey;
use sms_core::pack;
use sms_encoding::{analyse, normalise, transliterate_to_gsm7, SmsEncoding};
use sms_msisdn::{Msisdn, OperatorPrefixTable};

use crate::auth::{Principal, PrincipalKind};
use crate::cache::TtlCache;
use crate::pepper::{hmac_sha256_hex, HashPepper};
use crate::rbac::require_permission;
use crate::schema::{
    self, app, app_client, message, operator_prefix_rule, opt_out, provider, sender_id,
    sender_id_registration,
};

/// RSA modulus size for a freshly generated client keypair. Matches
/// `sms_auth::op::RSA_KEY_BITS` — same reasoning: the smallest size still
/// considered acceptable for RS256 in 2026, and this deployment has no need
/// for the extra headroom of 3072/4096 at the cost of a larger PEM the
/// caller has to store. Duplicated rather than imported: `sms-api` cannot
/// depend on `sms-auth` (the dependency runs the other way).
const CLIENT_RSA_KEY_BITS: usize = 2048;

/// Marker for a procedure whose backing subsystem is not built yet.
fn not_yet(procedure: &str, milestone: &str) -> CoolError {
    CoolError::Internal(format!(
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
            app_cache: std::sync::Arc::new(TtlCache::new(std::time::Duration::from_secs(60))),
            operator_cache: std::sync::Arc::new(TtlCache::new(std::time::Duration::from_secs(
                5 * 60,
            ))),
            pepper,
        }
    }

    /// `{HASH_SCHEME}:{hex}` — see `pepper.rs`'s `hmac_sha256_hex` for the
    /// algorithm. The one place `sendMessage` turns a plaintext MSISDN or
    /// body into the value persisted in `msisdnHash`/`bodyHash`.
    fn keyed_hash_hex(&self, input: &str) -> String {
        hmac_sha256_hex(&self.pepper, input)
    }

    /// A `system`-role context — the only one `OauthClient`-adjacent app
    /// resolution and the send path's own reads and writes admit. Built
    /// fresh per call rather than cached: it carries no state worth
    /// reusing, and constructing it is a handful of `Value::String`
    /// allocations, not a query.
    fn sys() -> CoolContext {
        Principal {
            sub: "sms-api:procedures".to_owned(),
            kind: PrincipalKind::App,
            role: "system".to_owned(),
            app_id: String::new(),
        }
        .into_context()
    }

    /// Analyse a body, and normalise a recipient if one was supplied.
    ///
    /// Runs [`normalise`] before [`analyse`], because normalisation is
    /// unconditional on the send path — previewing the raw body would quote a
    /// segment count the caller will never actually be billed for.
    fn preview(args: &schema::PreviewInput) -> Result<schema::PreviewResult, CoolError> {
        let normalised = normalise(&args.body);
        let report = analyse(&normalised);

        let normalized_to = args
            .to
            .as_deref()
            .filter(|to| !to.trim().is_empty())
            .map(|to| {
                Msisdn::parse_mobile(to)
                    .map(|m| m.as_e164().to_owned())
                    .map_err(|error| CoolError::Validation(error.to_string()))
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
            // policy denies read to anything but owner/admin/operator/
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
        db: &schema::Cratestack,
        sys: &CoolContext,
        client_id: String,
    ) -> Result<schema::App, CoolError> {
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
                    .ok_or_else(|| CoolError::Unauthorized("unknown client".to_owned()))?;

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
                    .ok_or_else(|| CoolError::Unauthorized("app not found or inactive".to_owned()))
            })
            .await
    }

    /// The cached prefix table, rebuilt from `OperatorPrefixRule` on a
    /// cache miss.
    async fn operator_table(
        &self,
        db: &schema::Cratestack,
        sys: &CoolContext,
    ) -> Result<OperatorPrefixTable, CoolError> {
        self.operator_cache
            .get_or_fetch((), |()| async move {
                let rows = db
                    .operator_prefix_rule()
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
        db: &schema::Cratestack,
        sys: &CoolContext,
        msisdn: &Msisdn,
    ) -> Result<schema::OperatorCode, CoolError> {
        let table = self.operator_table(db, sys).await?;
        Ok(table
            .lookup(msisdn)
            .and_then(parse_operator_code)
            .unwrap_or(schema::OperatorCode::unknown))
    }

    /// §3.2 step: opt-out check, on the hashed MSISDN, before anything else
    /// that would justify persisting a row.
    async fn ensure_not_opted_out(
        db: &schema::Cratestack,
        sys: &CoolContext,
        msisdn_hash: &str,
    ) -> Result<(), CoolError> {
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
            Err(CoolError::Validation(
                "recipient has opted out of messages from this scope".to_owned(),
            ))
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
        db: &schema::Cratestack,
        sys: &CoolContext,
        app: &schema::App,
        now: DateTime<Utc>,
    ) -> Result<(), CoolError> {
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
            Err(CoolError::Validation(format!(
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
        db: &schema::Cratestack,
        sys: &CoolContext,
        app: &schema::App,
        requested: Option<&str>,
    ) -> Result<String, CoolError> {
        const APPROVED: &str = "approved";

        let value = if let Some(value) = requested {
            value.to_owned()
        } else {
            let default_id = app.defaultSenderIdId.clone().ok_or_else(|| {
                CoolError::Validation(
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
                    CoolError::Validation(
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
                CoolError::Validation(format!("sender id {value:?} is not a registered sender"))
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
            Err(CoolError::Validation(format!(
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
        db: &schema::Cratestack,
        sys: &CoolContext,
        segments: i64,
    ) -> Result<Decimal, CoolError> {
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
    fn caller_client_id(ctx: &CoolContext) -> Result<String, CoolError> {
        let kind = match ctx.auth_field("kind") {
            Some(Value::String(kind)) => kind.as_str(),
            _ => return Err(CoolError::Unauthorized("missing kind claim".to_owned())),
        };
        if kind != PrincipalKind::App.as_str() {
            return Err(CoolError::Validation(
                "sendMessage currently requires a machine (client_credentials) caller — \
                 deriving an App for a human caller has no design yet"
                    .to_owned(),
            ));
        }
        match ctx.auth_field("sub") {
            Some(Value::String(sub)) => Ok(sub.clone()),
            _ => Err(CoolError::Unauthorized("missing sub claim".to_owned())),
        }
    }

    /// §3.2/§32: the nine pre-persistence steps. A message that reaches the
    /// database is one already decided to be sendable — this procedure's
    /// job is that decision, not delivery.
    async fn send(
        &self,
        db: &schema::Cratestack,
        ctx: &CoolContext,
        args: schema::SendMessageInput,
    ) -> Result<schema::SendMessageResult, CoolError> {
        // Layer 2 (#24, §5.1), checked before anything else runs. Layer 1's
        // own `@allow` on `sendMessage` admits any authenticated
        // `kind == "app"` caller unconditionally (schema.cstack:
        // `auth().kind == "app" || hasRole('owner') || hasRole('admin') ||
        // hasRole('operator')`) — nothing about *role* narrows a machine
        // caller down further, so this is the only thing standing between
        // "any client_credentials token this OP ever issues" and "sends
        // SMS", per §5.2's service-account scope table (`sms:send`).
        require_permission(ctx, "sms:send")?;

        let sys = Self::sys();
        let now = Utc::now();

        // 1. client_id -> App (cached).
        let client_id = Self::caller_client_id(ctx)?;
        let app = self.resolve_app(db, &sys, client_id).await?;

        // 2. MSISDN normalisation. `parse_mobile` rejects fixed-line and
        // unallocated ranges — failing here beats failing on a DLR later.
        let msisdn = Msisdn::parse_mobile(&args.to)
            .map_err(|error| CoolError::Validation(error.to_string()))?;
        let msisdn_hash = self.keyed_hash_hex(msisdn.as_e164());

        // 3. Opt-out check, before anything else persists.
        Self::ensure_not_opted_out(db, &sys, &msisdn_hash).await?;

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
        let operator = self.classify_operator(db, &sys, &msisdn).await?;

        // 8. Idempotency: the DB-level defence described in §4.5 as
        // independent of the HTTP `Idempotency-Key` layer, which never
        // reaches procedure code at all. `clientRef` is the only
        // caller-supplied correlation string `SendMessageInput` carries,
        // so it doubles as `idempotencyKey` when present; a caller that
        // supplies neither gets no DB-level dedupe, the same way a caller
        // that skips the `Idempotency-Key` header gets no HTTP-level
        // replay protection (§4.5: "document loudly").
        let idempotency_key = args.clientRef.clone();

        // 9. Cost estimate for the response — see `estimate_cost`'s own
        // doc for why this is a quote, not a routing decision.
        let segments = i64::from(report.segments);
        let estimated_cost = Self::estimate_cost(db, &sys, segments).await?;

        let class = args.class.unwrap_or(schema::MessageClass::transactional);
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
                maxAttempts: 3,
                leaseOwner: None,
                leaseUntil: None,
                scheduledAt: args.scheduledAt,
                expiresAt: expires_at,
                submittedAt: None,
                finalizedAt: None,
            })
            .run(&sys)
            .await?;

        Ok(schema::SendMessageResult {
            messageId: message.id,
            state: message.state,
            encoding: message.encoding,
            segments: message.segments,
            operator: message.operator,
            estimatedCostXaf: estimated_cost,
        })
    }

    /// #23: mint a fresh `AppClient` + `OauthClient` pair bound to
    /// `private_key_jwt`, and hand back the generated private key exactly
    /// once. No column anywhere holds it after this call returns — see
    /// `OauthClient` in `schema.cstack` and `sms_auth::to_registration`'s
    /// own comment on why no shared secret exists in this system (#6). This
    /// is the asymmetric analogue of "secret shown once."
    ///
    /// Both rows are written in one `serializable` transaction (the
    /// procedure's own `@isolation`, §2.10): `OauthClient.appClientId`
    /// references `AppClient.id`, and a caller must never observe one row
    /// without the other — either the client is fully provisioned or it
    /// does not exist yet.
    ///
    /// **Retirement / overlap-window rotation is out of scope for this
    /// procedure.** `OauthClient.update` is `hasRole('system')` only today
    /// — there is no owner/admin-reachable write path to revoke or rotate a
    /// client's key through the generated API yet, and a *true*
    /// overlap-window rotation (publishing an old and a new public key
    /// simultaneously, the way `sms_auth::op::rotate_signing_key` does for
    /// the OP's own key) needs a per-client key-history model shaped like
    /// `OauthSigningKey` — `OauthClient.jwks` holds exactly one JWK Set
    /// per client today, not a history of them. Deactivating a client
    /// (`AppClient.active` / `retiredAt`, already modelled) through the
    /// generated `update` route is owner/admin-reachable today and, while
    /// coarser than a real overlap window, is the honest scope this
    /// procedure adds: it does not itself expose a bespoke retire/rotate
    /// action. See #23's PR description for the full reasoning.
    async fn provision_client(
        &self,
        db: &schema::Cratestack,
        _ctx: &CoolContext,
        args: schema::ProvisionClientInput,
    ) -> Result<schema::ProvisionClientResult, CoolError> {
        let sys = Self::sys();

        // The app must exist and be active before a client is allowed to
        // speak for it — provisioning a client for a deleted/deactivated
        // app would silently mint working credentials for nothing.
        let app = db
            .app()
            .find_many()
            .where_expr(FilterExpr::from(app::id().eq(args.appId.clone())))
            .limit(1)
            .run(&sys)
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| CoolError::NotFound(format!("no App with id {}", args.appId)))?;
        if !app.active {
            return Err(CoolError::Validation(format!(
                "App {} is not active",
                app.id
            )));
        }

        // clientId is caller-visible immediately (it's what the token
        // endpoint's `client_id` parameter and the assertion's `iss`/`sub`
        // both carry), so it's generated here rather than left to a DB
        // default — a `Cuid` default would also exclude it from
        // CreateXInput entirely (§2.0). Not a `Cuid` itself: `AppClient`/
        // `OauthClient.clientId` are plain `String @length(min: 8, max:
        // 64)`, so there's no `[a-z0-9]{2,32}` format guard to satisfy.
        let client_id = format!("appc_{}", cratestack::uuid::Uuid::new_v4().simple());

        // Generate the client's own RSA keypair server-side. Only the
        // public half is ever persisted (below); the private half is
        // returned to the caller and never stored anywhere.
        let mut rng = OsRng;
        let key = RsaPrivateKey::new(&mut rng, CLIENT_RSA_KEY_BITS)
            .map_err(|error| CoolError::Internal(format!("generating client keypair: {error}")))?;
        let private_key_pem = key.to_pkcs8_pem(LineEnding::LF).map_err(|error| {
            CoolError::Internal(format!("encoding client key to PKCS#8 PEM: {error}"))
        })?;

        // `TokenManager::new_asymmetric` + `public_jwk()` derives n/e from
        // the key without hand-rolling base64url encoding — the same
        // pattern `sms_auth::op::load_signing_keys` uses for the OP's own
        // key. `authkestra_op::client_assertion::verify_client_assertion`
        // reads exactly this shape at the token endpoint: a JWK Set
        // (`{"keys": [...]}`) — see `select_key` in that module, vendored
        // source checked directly rather than assumed.
        let manager =
            TokenManager::new_asymmetric(private_key_pem.as_bytes(), None, Some(client_id.clone()))
                .map_err(|error| {
                    CoolError::Internal(format!("deriving the client's public JWK: {error}"))
                })?;
        let public_jwk = manager.public_jwk().ok_or_else(|| {
            CoolError::Internal(
                "TokenManager produced no public JWK for a freshly generated asymmetric key"
                    .to_owned(),
            )
        })?;
        let jwks_json = serde_json::json!({ "keys": [public_jwk] }).to_string();

        let scopes_packed =
            pack(&args.scopes).map_err(|error| CoolError::Validation(error.to_string()))?;
        // client_credentials only — the only grant type a machine caller in
        // this system ever uses (§4.2; see `sms_auth::op`'s own module doc).
        let grant_types_packed =
            pack(["client_credentials"]).expect("a static literal always packs");

        let app_id = args.appId;
        let label = args.label;

        run_in_isolated_tx(db.pool(), TransactionIsolation::Serializable, |mut tx| {
            let sys = &sys;
            let client_id = client_id.clone();
            let app_id = app_id.clone();
            let label = label.clone();
            let scopes_packed = scopes_packed.clone();
            let grant_types_packed = grant_types_packed.clone();
            let jwks_json = jwks_json.clone();
            async move {
                let app_client = db
                    .app_client()
                    .create(schema::CreateAppClientInput {
                        appId: app_id,
                        clientId: client_id.clone(),
                        label,
                        scopes: scopes_packed.clone(),
                        lastUsedAt: None,
                        retiredAt: None,
                    })
                    .run_in_tx(&mut tx, sys)
                    .await?;

                db.oauth_client()
                    .create(schema::CreateOauthClientInput {
                        clientId: client_id,
                        appClientId: Some(app_client.id),
                        tokenEndpointAuthMethod: schema::ClientAuthMethod::private_key_jwt,
                        jwks: Some(jwks_json),
                        grantTypes: grant_types_packed,
                        scopes: scopes_packed,
                        redirectUris: sms_core::EMPTY.to_owned(),
                        requirePkce: false,
                    })
                    .run_in_tx(&mut tx, sys)
                    .await?;

                Ok(((), tx))
            }
        })
        .await?;

        Ok(schema::ProvisionClientResult {
            clientId: client_id,
            privateKeyPem: private_key_pem.to_string(),
        })
    }
}

impl schema::procedures::ProcedureRegistry for Procedures {
    fn preview_message(
        &self,
        _db: &schema::Cratestack,
        _ctx: &CoolContext,
        args: schema::procedures::preview_message::Args,
    ) -> impl core::future::Future<
        Output = Result<schema::procedures::preview_message::Output, CoolError>,
    > + Send {
        core::future::ready(Self::preview(&args.args))
    }

    fn send_message(
        &self,
        db: &schema::Cratestack,
        ctx: &CoolContext,
        args: schema::procedures::send_message::Args,
    ) -> impl core::future::Future<
        Output = Result<schema::procedures::send_message::Output, CoolError>,
    > + Send {
        self.send(db, ctx, args.args)
    }

    fn list_messages_page(
        &self,
        _db: &schema::Cratestack,
        _ctx: &CoolContext,
        _args: schema::procedures::list_messages_page::Args,
    ) -> impl core::future::Future<
        Output = Result<schema::procedures::list_messages_page::Output, CoolError>,
    > + Send {
        core::future::ready(Err(not_yet("listMessagesPage", "milestone 2")))
    }

    fn cancel_message(
        &self,
        _db: &schema::Cratestack,
        _ctx: &CoolContext,
        _args: schema::procedures::cancel_message::Args,
    ) -> impl core::future::Future<
        Output = Result<schema::procedures::cancel_message::Output, CoolError>,
    > + Send {
        core::future::ready(Err(not_yet("cancelMessage", "milestone 2")))
    }

    fn enqueue_job(
        &self,
        _db: &schema::Cratestack,
        _ctx: &CoolContext,
        _args: schema::procedures::enqueue_job::Args,
    ) -> impl core::future::Future<Output = Result<schema::procedures::enqueue_job::Output, CoolError>>
           + Send {
        core::future::ready(Err(not_yet("enqueueJob", "milestone 2 (the jobs role)")))
    }

    fn provision_app_client(
        &self,
        db: &schema::Cratestack,
        ctx: &CoolContext,
        args: schema::procedures::provision_app_client::Args,
    ) -> impl core::future::Future<
        Output = Result<schema::procedures::provision_app_client::Output, CoolError>,
    > + Send {
        self.provision_client(db, ctx, args.args)
    }

    fn rotate_webhook_secret(
        &self,
        _db: &schema::Cratestack,
        _ctx: &CoolContext,
        _args: schema::procedures::rotate_webhook_secret::Args,
    ) -> impl core::future::Future<
        Output = Result<schema::procedures::rotate_webhook_secret::Output, CoolError>,
    > + Send {
        core::future::ready(Err(not_yet(
            "rotateWebhookSecret",
            "milestone 3 (webhooks)",
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn preview(body: &str, to: Option<&str>) -> Result<schema::PreviewResult, CoolError> {
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
        assert!(result.offending.is_empty());
    }

    #[test]
    fn preview_normalises_before_it_measures() {
        // The raw body is UCS-2 because of the typographic apostrophe. The send
        // path would normalise it away, so the preview must too — quoting UCS-2
        // here would overstate the bill on every message with a smart quote.
        let result = preview("Bienvenue sur l\u{2019}application", None).unwrap();
        assert_eq!(result.encoding, schema::Encoding::gsm7);
        assert!(result.offending.is_empty());
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
        assert!(matches!(error, CoolError::Validation(_)));
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
