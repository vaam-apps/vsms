#![doc = include_str!("lib.md")]

use std::collections::HashMap;
use std::fmt;
use std::str::FromStr;
use std::sync::Arc;

use sms_api::schema::Cratestack;
use sms_provider::SmsProvider;
use tokio_util::sync::CancellationToken;

pub mod claim;
pub mod dispatch;
pub mod drain;
pub mod hooks;
pub mod jobs;
pub mod lease;
pub mod routing;
pub mod scheduler;

/// A provider adapter registry, keyed by [`SmsProvider::key`] — matching
/// `Provider.key`, the column [`routing::decide`] resolves a chosen
/// `Route`'s provider back onto before `dispatch` can submit through it.
/// `Arc`-wrapped as a whole (not per-entry only) so cloning a
/// [`WorkerContext`] into a fresh `tokio::spawn` task is one pointer copy,
/// not a fresh `HashMap` allocation per clone.
pub type ProviderRegistry = Arc<HashMap<String, Arc<dyn SmsProvider>>>;

/// What a role's real body needs beyond its own lease/claim mechanics.
/// Cheap to clone — `Cratestack` wraps a pooled connection, [`ProviderRegistry`]
/// is a pointer — so every role task gets its own owned copy rather than
/// sharing borrows across `tokio::spawn` boundaries.
///
/// A registry, not a single provider: #62's routing engine can pick any
/// configured `Route`'s provider, so `dispatch` needs to resolve whichever
/// one a given message was actually routed to, not submit everything
/// through one hardcoded adapter. `backends/apps/sms-worker/src/main.rs` builds this
/// with exactly one entry today (`"orange_cm"`) — the same set of real
/// adapters this deployment has credentials for, unrelated to how many
/// `Provider` *rows* or `Route` rows exist in the database.
#[derive(Clone)]
pub struct WorkerContext {
    /// The pooled connection every role's queries run against.
    pub db: Cratestack,
    /// Every provider adapter this process holds credentials for. `dispatch`
    /// resolves the one a routed message needs by `Provider.key`; see
    /// [`ProviderRegistry`]'s own doc.
    pub providers: ProviderRegistry,
}

/// A role `sms-worker` can run. §7.1's table, verbatim.
///
/// Deliberately six variants, not the four this milestone actually builds —
/// `smpp` (M7) and the M3 shape of `hooks` don't exist yet, but the role
/// *name* is part of the operator-facing `--roles` contract from day one, so
/// `sms-worker --roles hooks,jobs` (the scaled-out node in §9.2's deployment
/// diagram) parses correctly now and gains real behaviour under it later,
/// rather than needing a flag-parsing change when hooks lands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
    /// Claim `messages` → route → submit. Singleton: Orange's TPS cap is
    /// per contract, not per process.
    Dispatch,
    /// `db.events().drain()` on an interval. Singleton: the framework runs
    /// no background drain worker, and multiple drainers multiply duplicate
    /// delivery (§8.2).
    Drain,
    /// Enqueue due recurring `Job` rows. Singleton: two schedulers
    /// double-enqueue (`jobs_dedupe_idx` catches it, but avoiding the race
    /// is cleaner than relying on the index to clean up after it).
    Scheduler,
    /// Claim `webhook_attempts` → signed POST. Scale to N: slow customer
    /// endpoints are the bottleneck, and parallelism is the fix.
    Hooks,
    /// Claim `jobs` → execute by `kind`. Scale to N: generic background
    /// work, nothing shared between rows.
    Jobs,
    /// Hold SMPP binds, pump `submit_sm`/`deliver_sm`. Singleton per
    /// provider: binds are stateful, sequence-numbered, and contractually
    /// count-limited. Milestone 7 — see `docs/architecture.md`'s open
    /// question on whether direct SMPP exists at all.
    Smpp,
}

/// Every role, in the order §7.1's table lists them. `--roles` accepts any
/// subset; this is what `sms-worker roles` prints and what tests iterate.
pub const ALL: [Role; 6] = [
    Role::Dispatch,
    Role::Drain,
    Role::Scheduler,
    Role::Hooks,
    Role::Jobs,
    Role::Smpp,
];

/// Whether a role tolerates more than one instance running at once.
///
/// Not a property of the role's *code* — every role in this crate is
/// ordinary async Rust with no shared mutable state of its own. It's a
/// property of what the role talks to: a provider's rate contract, a
/// stateful protocol session, or the framework's own delivery semantics.
/// §7.1 names the constraint for each variant; see [`Role::cardinality`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cardinality {
    /// At most one live instance cluster-wide, enforced by `lease.rs`'s
    /// advisory lock (#28). A second instance still starts — it just holds
    /// no lock until the first one dies, per §7.2's warm-standby design.
    Singleton,
    /// Any number of instances may run this role concurrently; they contend
    /// only in the microsecond window `claim.rs`'s optimistic CAS covers
    /// (#29).
    ScaleToN,
}

impl Role {
    /// The exact string `--roles` accepts and `sms-worker roles` prints —
    /// lowercase, matching every role name already used in prose throughout
    /// the design doc and the CLI examples in §7 and §9.2.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Role::Dispatch => "dispatch",
            Role::Drain => "drain",
            Role::Scheduler => "scheduler",
            Role::Hooks => "hooks",
            Role::Jobs => "jobs",
            Role::Smpp => "smpp",
        }
    }

    /// §7.1's table, as code. See [`Cardinality`] for what the distinction
    /// controls.
    #[must_use]
    pub const fn cardinality(self) -> Cardinality {
        match self {
            Role::Dispatch | Role::Drain | Role::Scheduler | Role::Smpp => Cardinality::Singleton,
            Role::Hooks | Role::Jobs => Cardinality::ScaleToN,
        }
    }
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A `--roles` value that isn't one of §7.1's six names.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown role {0:?}; expected one of dispatch, drain, scheduler, hooks, jobs, smpp")]
pub struct UnknownRole(String);

impl FromStr for Role {
    type Err = UnknownRole;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        ALL.into_iter()
            .find(|role| role.as_str() == s)
            .ok_or_else(|| UnknownRole(s.to_owned()))
    }
}

/// Run one role until cancelled.
///
/// `Dispatch` (#33) is the first role with a real body — see [`dispatch`].
/// Every other role is still a stub: it logs once, names the story that
/// will give it real work, and then idles — not a `sleep` loop burning a
/// wakeup every tick, but a future that never resolves on its own, exactly
/// the shape each real claim-loop-driven version will have once #35/#39/#40
/// land. The caller (`backends/apps/sms-worker`) is what actually stops any of
/// these, by dropping the task on shutdown; nothing in here decides when
/// to exit — `dispatch::run` keeps that same contract deliberately, not
/// just the stubs.
///
/// Mirrors `backends/crates/sms-api/src/procedures.rs`'s `not_yet` in spirit for the
/// roles still stubbed: "clearly-labelled error naming the milestone that
/// will build it, rather than a plausible-looking stub that would pass a
/// smoke test and lie."
///
/// `worker` identifies this process to `claim::claim_batch` (logged on a
/// denied claim, stored in `leaseOwner`) — not used by the still-stubbed
/// roles, but part of every role's signature rather than added role by
/// role as each one starts claiming something.
pub async fn run(role: Role, ctx: WorkerContext, worker: &str) {
    match role {
        Role::Dispatch => {
            dispatch::run(ctx, worker).await;
            return;
        }
        Role::Jobs => {
            jobs::run(ctx, worker).await;
            return;
        }
        Role::Scheduler => {
            scheduler::run(ctx, worker).await;
            return;
        }
        Role::Drain => {
            drain::run(ctx, worker).await;
            return;
        }
        Role::Hooks => {
            hooks::run(ctx, worker).await;
            return;
        }
        // The only role left with no real body — M7, see `story_for`.
        Role::Smpp => {}
    }
    tracing::warn!(
        role = %role,
        cardinality = ?role.cardinality(),
        story = story_for(role),
        "role started with no work implemented yet"
    );
    std::future::pending::<()>().await;
}

/// Run a singleton role behind a [`lease::RoleLease`], retrying every
/// [`lease::RETRY_INTERVAL`] until cancelled.
///
/// Meaningless for a [`Cardinality::ScaleToN`] role — `backends/apps/sms-worker`'s
/// `main` is what routes by cardinality; this function doesn't check, it
/// just does what its name says with whatever `role` it's given.
///
/// Unlike [`run`], this **does** return — when `shutdown` is cancelled,
/// whether that happens before a lock was ever acquired, while standing by,
/// or while holding one. The holding case is the one that matters: on the
/// way out it calls [`lease::RoleLease::release`], which is what makes a
/// clean shutdown faster than the failover path §7.2 describes for a hard
/// kill (`kill -9` skips this function entirely — the lease's `Drop` and
/// Postgres's own session-lock semantics are what release it then; see
/// [`lease`]'s module doc for why that's still correct, just slower).
pub async fn run_singleton(
    role: Role,
    database_url: String,
    ctx: WorkerContext,
    worker: String,
    shutdown: CancellationToken,
) {
    // #70: the one writer of `sms_worker_singleton_lease_held{role}` — see
    // `sms_metrics`'s own module doc for why setting this to `0` here,
    // rather than only ever touching it once acquired, is the whole point.
    // A process that never calls `run_singleton` for `role` at all (it
    // isn't in that process's own `--roles`) never touches this gauge
    // either, which is the other half of the same design: the metric is
    // *absent* from that process's `/metrics`, not present-and-zero — an
    // operator who only started this process with `--roles hooks,jobs`
    // should never see it claim anything about `dispatch`.
    let lease_gauge = sms_metrics::SINGLETON_LEASE_HELD.with_label_values(&[role.as_str()]);
    lease_gauge.set(0);

    loop {
        let acquired = tokio::select! {
            () = shutdown.cancelled() => {
                tracing::info!(role = %role, "cancelled before acquiring a lock");
                return;
            }
            result = lease::RoleLease::try_acquire(&database_url, role, &worker) => result,
        };

        match acquired {
            Ok(Some(held)) => {
                tracing::info!(role = %role, "singleton lock acquired");
                lease_gauge.set(1);
                tokio::select! {
                    () = run(role, ctx.clone(), &worker) => unreachable!("run() idles forever and never returns"),
                    () = shutdown.cancelled() => {
                        tracing::info!(role = %role, "shutdown requested; releasing lock");
                        lease_gauge.set(0);
                        if let Err(error) = held.release().await {
                            tracing::error!(
                                role = %role, %error,
                                "explicit lock release failed; the connection closing on \
                                 process exit still releases it, just not as fast"
                            );
                        }
                        return;
                    }
                }
            }
            Ok(None) => {
                tracing::debug!(role = %role, "lock held elsewhere; standing by");
                lease_gauge.set(0);
            }
            Err(error) => {
                // The dangerous case: not "someone else has it" but "this
                // attempt itself failed". If every node hits this for the
                // same role, the role is unheld cluster-wide, which is
                // exactly what §28 says to alert on rather than let blend
                // into routine standby logging.
                tracing::error!(role = %role, %error, "failed attempting the lock; retrying");
                lease_gauge.set(0);
            }
        }

        tokio::select! {
            () = shutdown.cancelled() => return,
            () = tokio::time::sleep(lease::RETRY_INTERVAL) => {}
        }
    }
}

/// Which open story gives a role its real loop body. Not `pub`: this is
/// operational commentary for the startup log, not part of the crate's
/// contract — nothing should match on it. Every arm but `Smpp` is
/// unreachable from [`run`] now (`Dispatch`/`Drain`/`Scheduler`/`Jobs`/
/// `Hooks` all `return` before falling through to this call) — kept as a
/// full match rather than narrowed to `Smpp` alone so a *sixth* role added
/// later fails to compile here too, the same "must classify every variant"
/// discipline `Role::cardinality`'s own match already relies on, rather than
/// silently warning "no work implemented yet" against a role that in fact
/// has some.
const fn story_for(role: Role) -> &'static str {
    match role {
        Role::Dispatch => "#32 (sendMessage) / #33 (state machine in the worker)",
        Role::Drain => "M3 #39 (drain role)",
        // scheduler enqueues Job rows, jobs claims and runs them — same
        // story builds both halves of the queue.
        Role::Scheduler | Role::Jobs => "#35 (jobs role and the generic job queue)",
        Role::Hooks => "M3 #40 (hooks role)",
        Role::Smpp => "M7 (SMPP and direct interconnect)",
    }
}

#[cfg(test)]
mod tests {
    use super::{ALL, Cardinality, Role, WorkerContext};
    use std::str::FromStr;
    use std::sync::Arc;

    #[test]
    fn every_role_round_trips_through_its_string_form() {
        for role in ALL {
            assert_eq!(Role::from_str(role.as_str()), Ok(role));
        }
    }

    #[test]
    fn an_unknown_role_name_is_a_typed_error_not_a_panic() {
        assert!(Role::from_str("dispach").is_err());
        assert!(Role::from_str("").is_err());
        assert!(Role::from_str("Dispatch").is_err(), "case-sensitive");
    }

    /// §7.1's table, pinned so a role can't silently change cardinality —
    /// that table exists because each answer traces to an external
    /// constraint (a provider contract, a stateful session, the framework's
    /// delivery semantics), not to a judgement call this test should be
    /// free to drift against.
    #[test]
    fn cardinality_matches_the_design_docs_table() {
        let singleton = [Role::Dispatch, Role::Drain, Role::Scheduler, Role::Smpp];
        let scale_to_n = [Role::Hooks, Role::Jobs];

        for role in singleton {
            assert_eq!(role.cardinality(), Cardinality::Singleton, "{role}");
        }
        for role in scale_to_n {
            assert_eq!(role.cardinality(), Cardinality::ScaleToN, "{role}");
        }
    }

    #[test]
    fn all_lists_every_variant_exactly_once() {
        let mut seen: Vec<Role> = Vec::new();
        for role in ALL {
            assert!(!seen.contains(&role), "{role} listed twice in ALL");
            seen.push(role);
        }
        assert_eq!(seen.len(), 6, "a variant was added without updating ALL");
    }

    /// A provider that must never actually be called — the one role this
    /// test exercises (`Smpp`) is still a stub, so `run()` never reaches the
    /// branch that would touch it. Every other role has its own live tests
    /// now that it's a real body, not a stub — this test is about the one
    /// that still is.
    struct NeverCalledProvider;

    #[async_trait::async_trait]
    impl sms_provider::SmsProvider for NeverCalledProvider {
        fn key(&self) -> &str {
            unimplemented!("stub roles never call the provider")
        }
        fn capabilities(&self) -> sms_provider::Capabilities {
            unimplemented!("stub roles never call the provider")
        }
        async fn submit(
            &self,
            _req: &sms_provider::SubmitRequest,
        ) -> Result<sms_provider::SubmitAck, sms_provider::ProviderError> {
            unimplemented!("stub roles never call the provider")
        }
        fn parse_dlr(
            &self,
            _raw: &sms_provider::RawCallback,
        ) -> Result<Vec<sms_provider::DeliveryUpdate>, sms_provider::ProviderError> {
            unimplemented!("stub roles never call the provider")
        }
        async fn health(&self) -> sms_provider::Health {
            unimplemented!("stub roles never call the provider")
        }
    }

    fn unused_worker_context() -> WorkerContext {
        // A lazy pool only parses the URL — never connects — matching the
        // same pattern `sms-api`'s own `router.rs` test uses for the same
        // reason: this context is never actually touched by a stub role.
        let pool = cratestack::sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://unused:unused@127.0.0.1/none")
            .expect("a lazy pool only parses the URL");
        let providers: std::collections::HashMap<String, Arc<dyn sms_provider::SmsProvider>> =
            std::collections::HashMap::from([(
                "unused".to_owned(),
                Arc::new(NeverCalledProvider) as Arc<dyn sms_provider::SmsProvider>,
            )]);
        WorkerContext {
            db: sms_api::schema::Cratestack::builder(pool).build(),
            providers: Arc::new(providers),
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_stub_role_never_resolves_on_its_own() {
        // The real assertion is "run() doesn't return" — proven by racing it
        // against a generous timeout under a paused clock, which advances
        // instantly and would still time out if run() ever completed.
        // Smpp, not any of the other five: every other role has a real body
        // now (#33, #35, #39, #40) and is covered by its own tests instead.
        // Smpp is the only role still a pure std::future::pending stub (M7)
        // — and the only one safe to drive against
        // `unused_worker_context()`'s never-connecting lazy pool, since
        // (unlike Drain/Hooks) it never touches the pool at all.
        let outcome = tokio::time::timeout(
            std::time::Duration::from_hours(8760),
            super::run(Role::Smpp, unused_worker_context(), "test-worker"),
        )
        .await;
        assert!(outcome.is_err(), "run() resolved; a stub role must idle");
    }
}
