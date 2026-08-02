//! The worker as a library. One binary, `sms-worker`, runs one or more
//! [`Role`]s selected at startup — see §7.1 of the design doc.
//!
//! [`Role`]/[`run`] are #27's shape — a role-selectable binary — over
//! [`lease`]'s advisory-lock leader election (#28) and [`claim`]'s CAS claim
//! loop (#29). None of the three decide what a role *does* while holding its
//! lock and claiming rows — [`run`] is still every role's entire body, still
//! "nothing yet" until #30–#35 actually call into [`claim::claim_batch`].
//!
//! This crate depends on `cratestack` (for [`lease`]'s raw-`sqlx` R1
//! exception) and, since #29, `sms-api` (for the expanded schema
//! [`claim`] claims against) — `include_server_schema!` is still invoked
//! exactly once, in `sms-api`'s own `lib.rs`; linking its already-compiled
//! output here doesn't re-run that expansion.

use std::fmt;
use std::str::FromStr;

use tokio_util::sync::CancellationToken;

pub mod claim;
pub mod lease;

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
/// Every role is currently a stub: it logs once, names the story that will
/// give it real work, and then idles — not a `sleep` loop burning a wakeup
/// every tick, but a future that never resolves on its own, exactly the
/// shape the real claim-loop-driven version will have once #28–#35 land.
/// The caller (`app/sms-worker`) is what actually stops it, by dropping the
/// task on shutdown; nothing in here decides when to exit.
///
/// Mirrors `crates/sms-api/src/procedures.rs`'s `not_yet` in spirit:
/// "clearly-labelled error naming the milestone that will build it, rather
/// than a plausible-looking stub that would pass a smoke test and lie" —
/// the same reasoning applies to a role loop as to a procedure. The
/// difference is failure mode: a procedure stub returns an error per
/// request; a role stub has no per-request boundary to fail at, so it logs
/// once at startup and then genuinely does nothing, which is the honest
/// worker-shaped equivalent.
pub async fn run(role: Role) {
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
/// Meaningless for a [`Cardinality::ScaleToN`] role — `app/sms-worker`'s
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
pub async fn run_singleton(role: Role, database_url: String, shutdown: CancellationToken) {
    loop {
        let acquired = tokio::select! {
            () = shutdown.cancelled() => {
                tracing::info!(role = %role, "cancelled before acquiring a lock");
                return;
            }
            result = lease::RoleLease::try_acquire(&database_url, role) => result,
        };

        match acquired {
            Ok(Some(held)) => {
                tracing::info!(role = %role, "singleton lock acquired");
                tokio::select! {
                    () = run(role) => unreachable!("run() idles forever and never returns"),
                    () = shutdown.cancelled() => {
                        tracing::info!(role = %role, "shutdown requested; releasing lock");
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
            }
            Err(error) => {
                // The dangerous case: not "someone else has it" but "this
                // attempt itself failed". If every node hits this for the
                // same role, the role is unheld cluster-wide, which is
                // exactly what §28 says to alert on rather than let blend
                // into routine standby logging.
                tracing::error!(role = %role, %error, "failed attempting the lock; retrying");
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
/// contract — nothing should match on it.
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
    use super::{Cardinality, Role, ALL};
    use std::str::FromStr;

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

    #[tokio::test(start_paused = true)]
    async fn a_stub_role_never_resolves_on_its_own() {
        // The real assertion is "run() doesn't return" — proven by racing it
        // against a generous timeout under a paused clock, which advances
        // instantly and would still time out if run() ever completed.
        let outcome = tokio::time::timeout(
            std::time::Duration::from_secs(60 * 60 * 24 * 365),
            super::run(Role::Dispatch),
        )
        .await;
        assert!(outcome.is_err(), "run() resolved; a stub role must idle");
    }
}
