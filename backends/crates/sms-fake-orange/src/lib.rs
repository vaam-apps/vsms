//! A fault-injecting fake of Orange Cameroon's SMS HTTP API.
//!
//! Built to fuzz `vsms`'s message state machine for invariant violations —
//! the automatable complement to `docs/runbooks/36-handset-gate.md`, not a
//! replacement for it. **This crate cannot close #36**: it cannot tell you
//! Orange's real DLR payload shape, whether `receiptRequest` is genuinely
//! honoured, or whether a handset ever buzzes. What it buys is a permanent
//! regression net over the failure modes that *can* be modelled from public
//! `OneAPI`-family documentation and this repo's own hard-won findings about
//! how `OrangeCmProvider` classifies transport failures (§6.1/§6.2).
//!
//! # Why a participant, not a response stub
//!
//! "The SMS never arrived" is not a response — it's the *absence* of a
//! later callback. A fake that only answers the submit HTTP call can never
//! model that, or a DLR that arrives twice, out of order, for an unknown
//! reference, or racing the submit response it's nominally about. So this
//! crate owns three things, not one:
//!
//! 1. **Inbound stubbing** ([`FakeOrange::start`]) — the token endpoint and
//!    the submit endpoint, answered per [`fault::FaultPolicy`].
//! 2. **A DLR scheduler** — a background `tokio` task per scheduled
//!    [`fault::DlrStep`], independent of the submit HTTP response, that
//!    POSTs a real `deliveryInfoNotification` body to whatever URL the
//!    caller wired up as the gateway's `POST /dlr/{providerKey}` route.
//! 3. **A request ledger** ([`ledger::Ledger`]) — every submit call
//!    received, queryable by test code, so a test can prove "Orange
//!    received this reference exactly once" from the provider's own side
//!    rather than inferring it from this system's database.
//!
//! # Two test policies, not a spectrum — plus one for a long-lived process
//!
//! [`fault::FaultPolicy::Scripted`] is an exact, ordered sequence — what a
//! deterministic CI-gate test scripts to assert one specific outcome.
//! [`fault::FaultPolicy::Seeded`] is a seeded PRNG that draws a weighted mix
//! of realistic outcomes — reproducible by construction, since the same
//! seed replayed against the same call sequence always draws the same
//! decisions. Never unseeded randomness anywhere in this crate.
//!
//! Neither fits a process that outlives any one test: `Scripted` exhausts
//! and falls back to a bare accept-with-no-DLR, and `Seeded` is tuned for a
//! fuzz sweep's tail coverage, not a demo's happy path. See
//! [`fault::FaultPolicy::Always`] for the third policy that exists
//! specifically for `backends/apps/sms-fake-orange`.

mod fault;
mod ledger;

pub use fault::{DlrStatus, DlrStep, FaultPolicy, SubmitDecision, SubmitOutcome};
pub use ledger::{Ledger, SubmitRecord};

use std::net::TcpListener;
use std::sync::Arc;
use std::time::Duration;

use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

/// How the fake answers `POST /oauth/v3/token`. Kept separate from
/// [`FaultPolicy`] — token faults are a coarse, whole-session concern (a
/// revoked credential, not a per-message one), not something that varies
/// call to call the way a submit fault does.
#[derive(Debug, Clone, Copy)]
pub enum TokenPolicy {
    /// Every token request succeeds — the default for every fault mode
    /// except the one dedicated to this specific failure.
    Always,
    /// Every token request answers `401` — `OrangeCmProvider::access_token`
    /// surfaces this as `ProviderError::Permanent` (see
    /// `sms-provider-orange-cm`'s own `bad_credentials_at_the_token_endpoint_are_permanent`),
    /// which `dispatch::classify` fails the message outright for. Models
    /// "401 mid-flight (token refresh)" from the design brief's fault list:
    /// a credential that was valid at the last refresh and no longer is.
    AlwaysUnauthorized,
}

/// A fault-injecting fake of Orange's SMS HTTP API. One instance per test —
/// `wiremock::MockServer::start` binds an ephemeral port, so tests never
/// collide even run in parallel.
pub struct FakeOrange {
    server: MockServer,
    ledger: Arc<Ledger>,
}

impl FakeOrange {
    /// Starts the fake: mounts the token and submit endpoints, wires
    /// `policy` to decide every submit call's outcome and DLR plan, and
    /// points its DLR scheduler at `dlr_endpoint` — a full URL
    /// (`http://127.0.0.1:PORT/dlr/orange_cm`), not just a base, since the
    /// caller owns route shape and provider-key dispatch, matching
    /// `backends/apps/sms-gateway/src/dlr.rs`'s own `POST /dlr/{providerKey}`
    /// mounting.
    ///
    /// `sender_number` must match whatever `OrangeCmConfig::sender_number`
    /// the caller configures its `OrangeCmProvider` with (without a `tel:`
    /// prefix) — it's part of the submit path
    /// (`/smsmessaging/v1/outbound/tel:{sender_number}/requests`, §6.2),
    /// exactly like every other wiremock-backed test in this workspace
    /// already matches it explicitly rather than wildcarding the path.
    pub async fn start(
        policy: FaultPolicy,
        token_policy: TokenPolicy,
        dlr_endpoint: impl Into<String>,
        sender_number: &str,
    ) -> Self {
        let server = MockServer::start().await;
        Self::mount(server, policy, token_policy, dlr_endpoint, sender_number).await
    }

    /// Same as [`Self::start`], but binds `listener` instead of an ephemeral
    /// OS-assigned port. A test never needs this — parallel tests colliding
    /// on a port is exactly what the ephemeral default avoids — but a
    /// long-lived demo process does: it needs to publish a stable,
    /// documented address (a compose file, a runbook command) rather than a
    /// port picked fresh on every start. `backends/apps/sms-fake-orange` is the one
    /// caller.
    pub async fn start_on(
        listener: TcpListener,
        policy: FaultPolicy,
        token_policy: TokenPolicy,
        dlr_endpoint: impl Into<String>,
        sender_number: &str,
    ) -> Self {
        let server = MockServer::builder().listener(listener).start().await;
        Self::mount(server, policy, token_policy, dlr_endpoint, sender_number).await
    }

    /// Shared setup behind [`Self::start`] and [`Self::start_on`] — mounts
    /// the token and submit endpoints on an already-bound `server`.
    async fn mount(
        server: MockServer,
        policy: FaultPolicy,
        token_policy: TokenPolicy,
        dlr_endpoint: impl Into<String>,
        sender_number: &str,
    ) -> Self {
        let token_response = match token_policy {
            TokenPolicy::Always => ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "fake-orange-token",
                "expires_in": 3600,
            })),
            TokenPolicy::AlwaysUnauthorized => ResponseTemplate::new(401),
        };
        Mock::given(method("POST"))
            .and(path("/oauth/v3/token"))
            .respond_with(token_response)
            .mount(&server)
            .await;

        let ledger = Ledger::new();
        let policy = Arc::new(policy);
        let dlr_client = reqwest::Client::new();
        let dlr_endpoint = dlr_endpoint.into();
        let submit_path = format!("/smsmessaging/v1/outbound/tel:{sender_number}/requests");

        let responder = {
            let ledger = Arc::clone(&ledger);
            move |request: &Request| -> ResponseTemplate {
                let reference = extract_callback_data(request).unwrap_or_default();
                let decision = policy.next();
                ledger.record_submit(&reference, &decision.outcome, decision.response_delay);
                tracing::info!(
                    reference,
                    outcome = ?decision.outcome,
                    delay_ms = %decision.response_delay.as_millis(),
                    dlrs_planned = decision.dlr_plan.len(),
                    "fake orange: submit received"
                );

                for step in &decision.dlr_plan {
                    schedule_dlr(
                        Arc::clone(&ledger),
                        dlr_client.clone(),
                        dlr_endpoint.clone(),
                        step.clone(),
                        reference.clone(),
                    );
                }

                response_for(&decision, &reference)
            }
        };

        Mock::given(method("POST"))
            .and(path(submit_path))
            .respond_with(responder)
            .mount(&server)
            .await;

        Self { server, ledger }
    }

    /// The fake's base URL — feed straight into `OrangeCmConfig::base_url`.
    #[must_use]
    pub fn base_url(&self) -> String {
        self.server.uri()
    }

    /// The request ledger, shared with the fake's own responder — a
    /// snapshot taken any time during or after the test reflects every
    /// submit call received so far.
    #[must_use]
    pub fn ledger(&self) -> Arc<Ledger> {
        Arc::clone(&self.ledger)
    }
}

/// `receiptRequest.callbackData` from a submit request body —
/// `OrangeCmProvider::submit` always sets this to `SubmitRequest::reference`
/// (`Message.id`). `None` for a body this fake can't parse — never produced
/// by the real adapter, only possible if something upstream is badly wrong,
/// so a caller sees an empty-string reference (and an obviously-wrong
/// ledger entry) rather than a panic.
fn extract_callback_data(request: &Request) -> Option<String> {
    let body: serde_json::Value = request.body_json().ok()?;
    body.get("outboundSMSMessageRequest")?
        .get("receiptRequest")?
        .get("callbackData")?
        .as_str()
        .map(str::to_owned)
}

/// Spawns the background task that delivers one [`fault::DlrStep`] —
/// independent of the submit HTTP response, which is the whole point: this
/// task's own `sleep` is timed from the moment the submit request was
/// *received*, so a short-enough `step.delay` reliably lands before the
/// caller ever sees the submit response, reproducing "the DLR arrives
/// before the submit response is processed".
fn schedule_dlr(
    ledger: Arc<Ledger>,
    client: reqwest::Client,
    endpoint: String,
    step: fault::DlrStep,
    default_reference: String,
) {
    ledger.mark_dlr_pending();
    tokio::spawn(async move {
        tokio::time::sleep(step.delay).await;
        let reference = step.reference_override.unwrap_or(default_reference);
        let body = dlr_body(&reference, &step.status);
        match client.post(&endpoint).json(&body).send().await {
            Ok(response) => {
                tracing::info!(
                    endpoint,
                    reference,
                    status = %step.status.wire(),
                    http_status = response.status().as_u16(),
                    "fake orange: DLR posted"
                );
            }
            Err(error) => {
                // A test's own DLR endpoint going away mid-run (e.g. the
                // test process is shutting down) is not this crate's
                // problem to surface as a panic from a detached background
                // task — the caller's own invariant sweep is what notices a
                // DLR that never arrived.
                tracing::warn!(%error, endpoint, reference, "fake orange: DLR delivery failed");
            }
        }
        ledger.mark_dlr_settled();
    });
}

/// Orange's own `deliveryInfoNotification` shape, exactly what
/// `sms-provider-orange-cm`'s real `dlr::parse` expects — see that crate's
/// own module doc for the public `OneAPI` reference this is grounded in.
fn dlr_body(reference: &str, status: &DlrStatus) -> serde_json::Value {
    serde_json::json!({
        "deliveryInfoNotification": {
            "callbackData": reference,
            "deliveryInfo": [
                {"address": "tel:+237677000000", "deliveryStatus": status.wire()}
            ]
        }
    })
}

/// The submit response body Orange's real API returns on success — same
/// `outboundSMSMessageRequest.resourceReference.resourceURL` envelope
/// `OrangeCmProvider::submit` extracts a `resource_id` from.
fn accepted_body(reference: &str) -> serde_json::Value {
    serde_json::json!({
        "outboundSMSMessageRequest": {
            "resourceReference": {
                "resourceURL": format!("https://fake-orange.invalid/requests/res-{reference}")
            }
        }
    })
}

fn response_for(decision: &SubmitDecision, reference: &str) -> ResponseTemplate {
    let template = match decision.outcome {
        SubmitOutcome::Accepted => {
            ResponseTemplate::new(201).set_body_json(accepted_body(reference))
        }
        SubmitOutcome::AcceptedMalformedBody => {
            ResponseTemplate::new(201).set_body_string("not json")
        }
        SubmitOutcome::AcceptedMissingResourceUrl => {
            ResponseTemplate::new(201).set_body_json(serde_json::json!({
                "outboundSMSMessageRequest": {"resourceReference": {"resourceURL": ""}}
            }))
        }
        SubmitOutcome::RateLimited => ResponseTemplate::new(429),
        SubmitOutcome::ServerError => ResponseTemplate::new(503),
        SubmitOutcome::Rejected => {
            ResponseTemplate::new(400).set_body_string("rejected by fake orange")
        }
    };
    if decision.response_delay > Duration::ZERO {
        template.set_delay(decision.response_delay)
    } else {
        template
    }
}
