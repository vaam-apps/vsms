#![doc = include_str!("delivery_status.md")]

use chrono::{DateTime, Duration, Utc};
use cratestack::{CratestackContext, CratestackError, FilterExpr, Value};
use sms_msisdn::Msisdn;

use crate::auth::{PrincipalKind, system_context};
use crate::pepper::{HashPepper, hmac_sha256_hex};
use crate::schema::{self, delivery_lookup, delivery_receipt, message};

/// How many by-number lookups one caller may make per hour. **Self-imposed**:
/// no regulation or provider contract names a figure. The number is the cheapest
/// thing in this procedure to abuse (it answers "has this person been texted, and
/// did it land" for any number typed), so it is counted; ids and client references
/// are not, because they must already be known. A value to reconsider, not a rule.
pub const MSISDN_LOOKUPS_PER_HOUR: i64 = 60;

/// The default `limit`.
pub const DEFAULT_LIMIT: i64 = 10;
/// The largest `limit` a caller may ask for.
pub const MAX_LIMIT: i64 = 50;
/// The default window of a lookup by client reference or number.
pub const DEFAULT_WINDOW_DAYS: i64 = 7;
/// The widest window: `Message`'s retention period, so a wider one asks for rows
/// that are already purged.
pub const MAX_WINDOW_DAYS: i64 = 90;

/// A cuid is lowercase alphanumeric, 2 to 32 characters (`vsms-data-layer`).
fn is_cuid(value: &str) -> bool {
    (2..=32).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
}

fn invalid(message: impl Into<String>) -> CratestackError {
    CratestackError::Validation(message.into())
}

/// Who is asking, as far as the plan is concerned: a person (no app) or an app
/// key (one app).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Asker {
    /// A signed-in person; sees every app.
    Person,
    /// An app key; sees `app_id` only.
    App {
        /// The caller's own app.
        app_id: String,
    },
}

/// The one key a lookup used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Key {
    /// A message id.
    MessageId(String),
    /// A caller-chosen client reference (inside one app).
    ClientRef(String),
    /// A whole recipient number, normalised to E.164.
    Msisdn(String),
}

impl Key {
    const fn kind(&self) -> schema::DeliveryLookupKey {
        match self {
            Key::MessageId(_) => schema::DeliveryLookupKey::message_id,
            Key::ClientRef(_) => schema::DeliveryLookupKey::client_ref,
            Key::Msisdn(_) => schema::DeliveryLookupKey::msisdn,
        }
    }
}

/// A validated lookup: what to find, in whose scope, in which window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LookupPlan {
    /// The one key.
    pub key: Key,
    /// The app every row must belong to: the app key's own, or a person's
    /// optional filter. `Some` always for a client reference.
    pub app_id: Option<String>,
    /// Lower bound of `createdAt`, inclusive.
    pub since: Option<DateTime<Utc>>,
    /// Upper bound of `createdAt`, inclusive.
    pub until: Option<DateTime<Utc>>,
    /// Rows to return.
    pub limit: i64,
}

impl LookupPlan {
    /// Validate `args` for `asker` at `now`. Pure: no clock, no database.
    ///
    /// # Errors
    ///
    /// [`CratestackError::Validation`] naming the rule broken: not exactly one key,
    /// a malformed id, a client reference without an app, an app key naming
    /// another app, a partial number, a `limit` outside `1..=50`, a window that
    /// ends before it starts or spans more than 90 days.
    pub fn from_args(
        args: &schema::DeliveryStatusQuery,
        asker: &Asker,
        now: DateTime<Utc>,
    ) -> Result<Self, CratestackError> {
        let keys = [
            args.messageId.is_some(),
            args.clientRef.is_some(),
            args.msisdn.is_some(),
        ];
        if keys.iter().filter(|present| **present).count() != 1 {
            return Err(invalid(
                "name exactly one of messageId, clientRef or msisdn",
            ));
        }

        let app_id = match (asker, args.appId.as_deref()) {
            (Asker::App { app_id }, None) => Some(app_id.clone()),
            (Asker::App { app_id }, Some(named)) if named == app_id => Some(app_id.clone()),
            (Asker::App { .. }, Some(_)) => {
                return Err(invalid("an app key looks up its own app's messages only"));
            }
            (Asker::Person, named) => named.map(str::to_owned),
        };
        if let Some(app_id) = &app_id
            && !is_cuid(app_id)
        {
            return Err(invalid("appId is not an id"));
        }

        let key = if let Some(id) = &args.messageId {
            if !is_cuid(id) {
                return Err(invalid("messageId is not an id"));
            }
            Key::MessageId(id.clone())
        } else if let Some(reference) = &args.clientRef {
            if reference.trim().is_empty() {
                return Err(invalid("clientRef must not be blank"));
            }
            if app_id.is_none() {
                return Err(invalid(
                    "a clientRef belongs to one app: name it with appId",
                ));
            }
            Key::ClientRef(reference.clone())
        } else {
            let raw = args.msisdn.as_deref().unwrap_or_default();
            let number = Msisdn::parse_mobile(raw).map_err(|error| invalid(error.to_string()))?;
            Key::Msisdn(number.into_e164())
        };

        let limit = args.limit.unwrap_or(DEFAULT_LIMIT);
        if !(1..=MAX_LIMIT).contains(&limit) {
            return Err(invalid(format!("limit must be between 1 and {MAX_LIMIT}")));
        }

        let (since, until) = match (&key, args.since, args.until) {
            // An id is exact: bound it only if the caller asked.
            (Key::MessageId(_), since, until) => (since, until),
            (_, since, until) => {
                let until = until.unwrap_or(now);
                let since = since.unwrap_or(until - Duration::days(DEFAULT_WINDOW_DAYS));
                (Some(since), Some(until))
            }
        };
        if let (Some(since), Some(until)) = (since, until) {
            if since > until {
                return Err(invalid("since is after until"));
            }
            if until - since > Duration::days(MAX_WINDOW_DAYS) {
                return Err(invalid(format!(
                    "the window may not be wider than {MAX_WINDOW_DAYS} days"
                )));
            }
        }

        Ok(Self {
            key,
            app_id,
            since,
            until,
            limit,
        })
    }
}

/// The caller as read off the request context.
struct Caller {
    sub: String,
    role: String,
    asker: Asker,
}

impl Caller {
    fn of(ctx: &CratestackContext) -> Result<Self, CratestackError> {
        let field = |name: &str| match ctx.auth_field(name) {
            Some(Value::String(value)) => Some(value.clone()),
            _ => None,
        };
        let forbidden = |why: &str| CratestackError::Forbidden(why.to_owned());
        let sub = field("sub")
            .filter(|sub| !sub.is_empty())
            .ok_or_else(|| forbidden("a delivery lookup needs a caller identity"))?;
        let role = field("role").unwrap_or_default();
        let kind = field("kind").unwrap_or_default();
        let asker = if kind == PrincipalKind::App.as_str() {
            // An app key with no app would see every app's messages through the
            // system read below: refuse it rather than scope to nothing.
            let app_id = field("appId")
                .filter(|app_id| !app_id.is_empty())
                .ok_or_else(|| forbidden("an app key without an app cannot look up messages"))?;
            Asker::App { app_id }
        } else if kind == PrincipalKind::User.as_str() {
            Asker::Person
        } else {
            return Err(forbidden("a delivery lookup needs a person or an app key"));
        };
        Ok(Self { sub, role, asker })
    }

    const fn kind(&self) -> &'static str {
        match self.asker {
            Asker::Person => "user",
            Asker::App { .. } => "app",
        }
    }
}

/// The message filter a plan stands for. The key arm is always present, so a
/// plan can never become an unfiltered list.
fn message_filter(plan: &LookupPlan, pepper: &HashPepper) -> FilterExpr {
    let mut filter = match &plan.key {
        Key::MessageId(id) => FilterExpr::from(message::id().eq(id.clone())),
        Key::ClientRef(reference) => {
            FilterExpr::from(message::idempotencyKey().eq(reference.clone()))
        }
        Key::Msisdn(e164) => {
            FilterExpr::from(message::msisdnHash().eq(hmac_sha256_hex(pepper, e164)))
        }
    };
    if let Some(app_id) = &plan.app_id {
        filter = filter.and(message::appId().eq(app_id.clone()));
    }
    if let Some(since) = plan.since {
        filter = filter.and(message::createdAt().gte(since));
    }
    if let Some(until) = plan.until {
        filter = filter.and(message::createdAt().lte(until));
    }
    filter
}

/// What the audit row records as the key: never the thing typed, except an id.
fn audited_key(key: &Key, pepper: &HashPepper) -> String {
    match key {
        Key::MessageId(id) => id.clone(),
        Key::Msisdn(e164) => hmac_sha256_hex(pepper, e164),
        // A different input space from a number or a body, so this hash can never
        // equal a `msisdnHash` or `bodyHash`.
        Key::ClientRef(reference) => {
            hmac_sha256_hex(pepper, &format!("delivery-lookup:client_ref:{reference}"))
        }
    }
}

/// The masked recipient, or `None` for a purged message (its number is a
/// placeholder) and for a stored number that does not parse (never echoed raw).
fn recipient(row: &schema::Message) -> Option<String> {
    if row.purgedAt.is_some() {
        return None;
    }
    Msisdn::parse(&row.msisdn)
        .ok()
        .map(|number| number.masked())
}

fn summary(row: schema::DeliveryReceipt) -> schema::DeliveryReceiptSummary {
    schema::DeliveryReceiptSummary {
        id: row.id,
        providerId: row.providerId,
        outcome: row.outcome,
        rawStatus: row.rawStatus,
        errorCode: row.errorCode,
        networkCode: row.networkCode,
        receivedAt: row.receivedAt,
        occurredAt: row.occurredAt,
    }
}

/// One message's answer. The only place a `DeliveryStatus` is built, so the
/// fields it can carry are exactly the ones listed here.
fn status(
    row: schema::Message,
    last_receipt: Option<schema::DeliveryReceiptSummary>,
) -> schema::DeliveryStatus {
    let recipient = recipient(&row);
    schema::DeliveryStatus {
        messageId: row.id,
        appId: row.appId,
        recipient,
        operator: row.operator,
        senderIdValue: row.senderIdValue,
        class: row.class,
        encoding: row.encoding,
        segments: row.segments,
        state: row.state,
        attempts: row.attempts,
        acceptedAt: row.createdAt,
        scheduledAt: row.scheduledAt,
        submittedAt: row.submittedAt,
        finalizedAt: row.finalizedAt,
        expiresAt: row.expiresAt,
        purged: row.purgedAt.is_some(),
        lastReceipt: last_receipt,
    }
}

/// Refuse a number lookup past the caller's hourly budget.
async fn ensure_number_budget(
    db: &schema::Cratestack,
    sys: &CratestackContext,
    caller: &Caller,
    now: DateTime<Utc>,
) -> Result<(), CratestackError> {
    let used = db
        .delivery_lookup()
        .aggregate()
        .count()
        .where_expr(
            FilterExpr::from(delivery_lookup::actorSub().eq(caller.sub.clone()))
                .and(delivery_lookup::keyKind().eq(schema::DeliveryLookupKey::msisdn))
                .and(delivery_lookup::createdAt().gte(now - Duration::hours(1))),
        )
        .run(sys)
        .await?;
    if used >= MSISDN_LOOKUPS_PER_HOUR {
        return Err(CratestackError::Forbidden(format!(
            "delivery lookup budget exhausted: {MSISDN_LOOKUPS_PER_HOUR} number lookups per hour"
        )));
    }
    Ok(())
}

/// The newest receipt of each message, one read for all of them.
async fn newest_receipts(
    db: &schema::Cratestack,
    sys: &CratestackContext,
    ids: Vec<String>,
) -> Result<std::collections::HashMap<String, schema::DeliveryReceiptSummary>, CratestackError> {
    // `IN ()` is not SQL: no message found, nothing to ask.
    if ids.is_empty() {
        return Ok(std::collections::HashMap::new());
    }
    let rows = db
        .delivery_receipt()
        .find_many()
        .where_expr(FilterExpr::from(delivery_receipt::messageId().in_(ids)))
        .order_by(delivery_receipt::receivedAt().desc())
        .run(sys)
        .await?;
    let mut newest = std::collections::HashMap::new();
    for row in rows {
        newest
            .entry(row.messageId.clone())
            .or_insert_with(|| summary(row));
    }
    Ok(newest)
}

/// Run the lookup. The caller has already passed `require_permission(ctx,
/// "delivery:read")`: this is the body that follows it, so a test can drive it
/// with a fixed `now`.
///
/// # Errors
///
/// [`CratestackError::Validation`] for a malformed query,
/// [`CratestackError::Forbidden`] for a caller that is neither a person nor an
/// app key with an app, or one past its number budget, and whatever the database
/// answers, including when the audit row cannot be written: then nothing is
/// answered.
pub async fn lookup(
    pepper: &HashPepper,
    db: &schema::Cratestack,
    ctx: &CratestackContext,
    args: schema::DeliveryStatusQuery,
    now: DateTime<Utc>,
) -> Result<schema::DeliveryStatusResult, CratestackError> {
    let caller = Caller::of(ctx)?;
    let plan = LookupPlan::from_args(&args, &caller.asker, now)?;
    let sys = system_context("sms-api:delivery-lookup");

    if matches!(plan.key, Key::Msisdn(_)) {
        ensure_number_budget(db, &sys, &caller, now).await?;
    }

    let limit = usize::try_from(plan.limit).unwrap_or(usize::MAX);
    let mut rows = db
        .message()
        .find_many()
        .where_expr(message_filter(&plan, pepper))
        .order_by(message::createdAt().desc())
        .limit(plan.limit + 1)
        .run(&sys)
        .await?;
    let truncated = rows.len() > limit;
    rows.truncate(limit);

    let mut receipts =
        newest_receipts(db, &sys, rows.iter().map(|row| row.id.clone()).collect()).await?;
    let statuses: Vec<schema::DeliveryStatus> = rows
        .into_iter()
        .map(|row| {
            let last = receipts.remove(&row.id);
            status(row, last)
        })
        .collect();

    // Fail closed: a lookup that cannot be accounted for is not answered.
    db.delivery_lookup()
        .create(schema::CreateDeliveryLookupInput {
            actorSub: caller.sub.clone(),
            actorKind: caller.kind().to_owned(),
            actorRole: if caller.role.is_empty() {
                "none".to_owned()
            } else {
                caller.role.clone()
            },
            actorAppId: match &caller.asker {
                Asker::App { app_id } => Some(app_id.clone()),
                Asker::Person => None,
            },
            keyKind: plan.key.kind(),
            keyValue: audited_key(&plan.key, pepper),
            appIdFilter: plan.app_id.clone(),
            resultCount: i64::try_from(statuses.len()).unwrap_or(i64::MAX),
        })
        .run(&sys)
        .await?;

    Ok(schema::DeliveryStatusResult {
        statuses,
        truncated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> DateTime<Utc> {
        "2026-11-01T12:00:00Z".parse().unwrap()
    }

    fn app() -> Asker {
        Asker::App {
            app_id: "capp1".to_owned(),
        }
    }

    fn query() -> schema::DeliveryStatusQuery {
        schema::DeliveryStatusQuery {
            messageId: None,
            appId: None,
            clientRef: None,
            msisdn: None,
            since: None,
            until: None,
            limit: None,
        }
    }

    fn by_id() -> schema::DeliveryStatusQuery {
        schema::DeliveryStatusQuery {
            messageId: Some("cmsg1".to_owned()),
            ..query()
        }
    }

    fn by_number(number: &str) -> schema::DeliveryStatusQuery {
        schema::DeliveryStatusQuery {
            msisdn: Some(number.to_owned()),
            ..query()
        }
    }

    fn refusal(args: &schema::DeliveryStatusQuery, asker: &Asker) -> String {
        match LookupPlan::from_args(args, asker, now()) {
            Err(CratestackError::Validation(message)) => message,
            other => panic!("expected a validation refusal, got {other:?}"),
        }
    }

    #[test]
    fn no_key_and_two_keys_are_refused() {
        assert!(refusal(&query(), &Asker::Person).contains("exactly one"));
        let two = schema::DeliveryStatusQuery {
            msisdn: Some("+237677123456".to_owned()),
            ..by_id()
        };
        assert!(refusal(&two, &Asker::Person).contains("exactly one"));
        let three = schema::DeliveryStatusQuery {
            clientRef: Some("ref".to_owned()),
            ..two
        };
        assert!(refusal(&three, &app()).contains("exactly one"));
    }

    #[test]
    fn a_client_reference_belongs_to_an_app() {
        let args = schema::DeliveryStatusQuery {
            clientRef: Some("order-17".to_owned()),
            ..query()
        };
        // A person has to say which app; an app key's own is implied.
        assert!(refusal(&args, &Asker::Person).contains("appId"));
        let plan = LookupPlan::from_args(&args, &app(), now()).unwrap();
        assert_eq!(plan.app_id.as_deref(), Some("capp1"));
        let named = schema::DeliveryStatusQuery {
            appId: Some("capp2".to_owned()),
            ..args
        };
        let plan = LookupPlan::from_args(&named, &Asker::Person, now()).unwrap();
        assert_eq!(plan.app_id.as_deref(), Some("capp2"));
    }

    #[test]
    fn an_app_key_naming_another_app_is_refused_not_ignored() {
        let foreign = schema::DeliveryStatusQuery {
            appId: Some("capp2".to_owned()),
            ..by_id()
        };
        assert!(refusal(&foreign, &app()).contains("own app"));
        let own = schema::DeliveryStatusQuery {
            appId: Some("capp1".to_owned()),
            ..by_id()
        };
        assert_eq!(
            LookupPlan::from_args(&own, &app(), now()).unwrap().app_id,
            Some("capp1".to_owned())
        );
    }

    #[test]
    fn an_app_key_is_always_scoped_to_its_app_and_a_person_to_nothing_unless_asked() {
        for args in [by_id(), by_number("+237677123456")] {
            assert_eq!(
                LookupPlan::from_args(&args, &app(), now()).unwrap().app_id,
                Some("capp1".to_owned())
            );
            assert_eq!(
                LookupPlan::from_args(&args, &Asker::Person, now())
                    .unwrap()
                    .app_id,
                None
            );
        }
    }

    #[test]
    fn a_malformed_id_is_refused() {
        for bad in ["", "x", "C1", "has space", "a/b", &"a".repeat(33)] {
            let args = schema::DeliveryStatusQuery {
                messageId: Some(bad.to_owned()),
                ..query()
            };
            assert!(
                refusal(&args, &Asker::Person).contains("not an id"),
                "{bad}"
            );
        }
        let args = schema::DeliveryStatusQuery {
            appId: Some("Not An Id".to_owned()),
            ..by_id()
        };
        assert!(refusal(&args, &Asker::Person).contains("appId"));
    }

    #[test]
    fn a_blank_client_reference_is_refused() {
        let args = schema::DeliveryStatusQuery {
            clientRef: Some("   ".to_owned()),
            appId: Some("capp1".to_owned()),
            ..query()
        };
        assert!(refusal(&args, &Asker::Person).contains("blank"));
    }

    #[test]
    fn only_a_whole_number_is_looked_up() {
        for partial in ["+2376", "6771", "", "abc", "+237"] {
            let message = refusal(&by_number(partial), &Asker::Person);
            assert!(!message.is_empty(), "{partial}");
        }
        // The key is the normalised number, so `677123456` and `+237677123456` are one key.
        let a = LookupPlan::from_args(&by_number("677123456"), &Asker::Person, now()).unwrap();
        let b = LookupPlan::from_args(&by_number("+237677123456"), &Asker::Person, now()).unwrap();
        assert_eq!(a.key, Key::Msisdn("+237677123456".to_owned()));
        assert_eq!(a.key, b.key);
    }

    #[test]
    fn limit_is_one_to_fifty_and_defaults_to_ten() {
        assert_eq!(
            LookupPlan::from_args(&by_id(), &Asker::Person, now())
                .unwrap()
                .limit,
            10
        );
        for ok in [1, 50] {
            let args = schema::DeliveryStatusQuery {
                limit: Some(ok),
                ..by_id()
            };
            assert_eq!(
                LookupPlan::from_args(&args, &Asker::Person, now())
                    .unwrap()
                    .limit,
                ok
            );
        }
        for bad in [0, 51, -1, i64::MAX] {
            let args = schema::DeliveryStatusQuery {
                limit: Some(bad),
                ..by_id()
            };
            assert!(refusal(&args, &Asker::Person).contains("limit"), "{bad}");
        }
    }

    #[test]
    fn a_number_or_reference_gets_a_seven_day_window_and_an_id_gets_none() {
        let plan =
            LookupPlan::from_args(&by_number("+237677123456"), &Asker::Person, now()).unwrap();
        assert_eq!(plan.until, Some(now()));
        assert_eq!(plan.since, Some(now() - Duration::days(7)));
        let plan = LookupPlan::from_args(&by_id(), &Asker::Person, now()).unwrap();
        assert_eq!((plan.since, plan.until), (None, None));
    }

    #[test]
    fn the_window_may_not_exceed_ninety_days_or_run_backwards() {
        let wide = schema::DeliveryStatusQuery {
            since: Some(now() - Duration::days(91)),
            ..by_number("+237677123456")
        };
        assert!(refusal(&wide, &Asker::Person).contains("90 days"));
        let edge = schema::DeliveryStatusQuery {
            since: Some(now() - Duration::days(90)),
            ..by_number("+237677123456")
        };
        assert!(LookupPlan::from_args(&edge, &Asker::Person, now()).is_ok());
        let backwards = schema::DeliveryStatusQuery {
            since: Some(now()),
            until: Some(now() - Duration::days(1)),
            ..by_number("+237677123456")
        };
        assert!(refusal(&backwards, &Asker::Person).contains("after until"));
        // An id with a named window is held to the same rules.
        let id_wide = schema::DeliveryStatusQuery {
            since: Some(now() - Duration::days(400)),
            until: Some(now()),
            ..by_id()
        };
        assert!(refusal(&id_wide, &Asker::Person).contains("90 days"));
    }

    #[test]
    fn the_audited_key_is_never_the_number_or_the_reference_typed() {
        // Made for this run: no key material is written in the source.
        let pepper = HashPepper::new(format!(
            "{}{}",
            cratestack::uuid::Uuid::new_v4().simple(),
            cratestack::uuid::Uuid::new_v4().simple()
        ))
        .unwrap();
        let number = Key::Msisdn("+237677123456".to_owned());
        let hashed = audited_key(&number, &pepper);
        assert!(hashed.starts_with("hmac-sha256-v1:"));
        assert!(!hashed.contains("677123456"));
        let reference = Key::ClientRef("order-17".to_owned());
        let hashed_ref = audited_key(&reference, &pepper);
        assert!(hashed_ref.starts_with("hmac-sha256-v1:"));
        assert!(!hashed_ref.contains("order-17"));
        // Not the same input space as a bare string with that value.
        assert_ne!(hashed_ref, hmac_sha256_hex(&pepper, "order-17"));
        assert_eq!(
            audited_key(&Key::MessageId("cmsg1".to_owned()), &pepper),
            "cmsg1"
        );
    }
}
