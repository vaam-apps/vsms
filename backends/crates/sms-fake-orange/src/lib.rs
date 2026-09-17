#![doc = include_str!("lib.md")]

mod fault;
mod ledger;

pub use fault::{DlrStatus, DlrStep, FaultPolicy, SubmitDecision, SubmitOutcome};
pub use ledger::{Ledger, SubmitRecord};

use std::net::TcpListener;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
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
        // Mints one `resource_id` per submit call, exactly the way real
        // Orange mints its own `{{resource_id}}` per submission (§4 "About
        // SMS Delivery Receipt") — never derived from anything the caller
        // sent, since the documented request carries nothing this fake (or
        // real Orange) could use as a caller-chosen correlation token. See
        // `ledger.md`'s own module doc for why this is a *different*
        // concept from `SubmitRecord::to`.
        let next_resource_id = Arc::new(AtomicU64::new(1));

        let responder = {
            let ledger = Arc::clone(&ledger);
            move |request: &Request| -> ResponseTemplate {
                let to = extract_destination(request).unwrap_or_default();
                let resource_id =
                    format!("res-{}", next_resource_id.fetch_add(1, Ordering::SeqCst));
                let decision = policy.next();
                ledger.record_submit(
                    &to,
                    &resource_id,
                    &decision.outcome,
                    decision.response_delay,
                );
                tracing::info!(
                    to,
                    resource_id,
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
                        resource_id.clone(),
                    );
                }

                response_for(&decision, &resource_id)
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

/// The destination this submit request named, from the documented
/// `address` array (<https://developer.orange.com/apis/sms/getting-started>
/// — the "Getting Started" sample submit body). `tel:` scheme stripped so
/// it matches `Message.msisdn` verbatim. This is the only field on the
/// documented wire shape that could plausibly distinguish one logical
/// message from another — the real, documented request carries no
/// caller-supplied reference at all (see `sms-provider-orange-cm`'s own
/// `lib.rs` module doc on why `receiptRequest` is gone). `None` for a
/// body this fake can't parse — never produced by the real adapter, only
/// possible if something upstream is badly wrong, so a caller sees an
/// empty-string `to` (and an obviously-wrong ledger entry) rather than a
/// panic.
fn extract_destination(request: &Request) -> Option<String> {
    let body: serde_json::Value = request.body_json().ok()?;
    let raw = body
        .get("outboundSMSMessageRequest")?
        .get("address")?
        .as_array()?
        .first()?
        .as_str()?;
    Some(raw.trim_start_matches("tel:").to_owned())
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
    default_resource_id: String,
) {
    ledger.mark_dlr_pending();
    tokio::spawn(async move {
        tokio::time::sleep(step.delay).await;
        let resource_id = step.resource_id_override.unwrap_or(default_resource_id);
        let body = dlr_body(&resource_id, &step.status);
        match client.post(&endpoint).json(&body).send().await {
            Ok(response) => {
                tracing::info!(
                    endpoint,
                    resource_id,
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
                tracing::warn!(%error, endpoint, resource_id, "fake orange: DLR delivery failed");
            }
        }
        ledger.mark_dlr_settled();
    });
}

/// Orange's own `deliveryInfoNotification` shape — §4 "About SMS Delivery
/// Receipt" (<https://developer.orange.com/apis/sms/getting-started>):
/// `deliveryInfo` is a single JSON object, sibling to `callbackData`, not
/// an array (`sms-provider-orange-cm`'s real `dlr::parse` accepts both
/// leniently, but this fake emits exactly what's documented). `callbackData`
/// carries Orange's own `{{resource_id}}` — the same id the accompanying
/// submit's `201` echoed back as `resourceURL`'s trailing segment, per
/// `accepted_body` below. `address` is real, documented data too, so it's
/// included even though nothing reads it — a fixed placeholder is fine,
/// it plays no correlation role.
fn dlr_body(resource_id: &str, status: &DlrStatus) -> serde_json::Value {
    serde_json::json!({
        "deliveryInfoNotification": {
            "callbackData": resource_id,
            "deliveryInfo": {
                "address": "tel:+237677000000",
                "deliveryStatus": status.wire()
            }
        }
    })
}

/// The submit response body Orange's real API returns on success (captured
/// live, #95): `resourceURL` sits DIRECTLY inside `outboundSMSMessageRequest`,
/// NOT nested under a `resourceReference` wrapper as the public `OneAPI` docs
/// describe. `resource_id` is the id this fake minted for the call (see
/// `mount`'s own responder), never a caller-supplied one — Orange's own
/// docs (§4) frame `{{resource_id}}` as Orange's own value, communicated
/// to the caller for the first time in this very response.
fn accepted_body(resource_id: &str) -> serde_json::Value {
    serde_json::json!({
        "outboundSMSMessageRequest": {
            "resourceURL": format!("https://fake-orange.invalid/requests/{resource_id}")
        }
    })
}

fn response_for(decision: &SubmitDecision, resource_id: &str) -> ResponseTemplate {
    let template = match decision.outcome {
        SubmitOutcome::Accepted => {
            ResponseTemplate::new(201).set_body_json(accepted_body(resource_id))
        }
        SubmitOutcome::AcceptedMalformedBody => {
            ResponseTemplate::new(201).set_body_string("not json")
        }
        SubmitOutcome::AcceptedMissingResourceUrl => {
            ResponseTemplate::new(201).set_body_json(serde_json::json!({
                "outboundSMSMessageRequest": {"resourceURL": ""}
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
