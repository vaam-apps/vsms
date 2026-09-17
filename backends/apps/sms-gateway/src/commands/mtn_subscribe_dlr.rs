//! `Command::MtnSubscribeDlr` — registers this deployment's DLR endpoint
//! with MADAPI. See that variant's own doc comment in `main.rs` for why
//! this exists as an operator action rather than something `serve` does
//! on startup.

use anyhow::{Context, Result};
use sms_provider_mtn::{MtnConfig, MtnProvider};

/// `Command::MtnSubscribeDlr`'s flags. The enum variant carries the
/// "why"; this struct carries only the flags themselves, matching
/// `RecordRouteValidationArgs`' own split.
///
/// Deliberately takes MTN's credentials as its own flags rather than
/// reading `serve`'s: this command is run once, by hand, against an
/// account that may not be the one a running gateway holds — and it
/// needs no database at all, which is why there is no `--database-url`
/// here unlike every other subcommand in this module.
#[derive(Debug, clap::Args)]
pub(crate) struct MtnSubscribeDlrArgs {
    #[arg(long, env = "MTN_CLIENT_ID")]
    pub(crate) mtn_client_id: String,

    #[arg(long, env = "MTN_CLIENT_SECRET", hide_env_values = true)]
    pub(crate) mtn_client_secret: String,

    /// The approved short code this subscription is registered against.
    /// MADAPI scopes a subscription to a `serviceCode`, so a deployment
    /// sending under two short codes needs this run once per code.
    #[arg(long, env = "MTN_SERVICE_CODE")]
    pub(crate) mtn_service_code: String,

    #[arg(long, env = "MTN_BASE_URL", default_value = "https://api.mtn.com")]
    pub(crate) mtn_base_url: String,

    /// The publicly reachable HTTPS URL MTN should POST delivery
    /// receipts to — this deployment's own `POST /dlr/{providerKey}`,
    /// with `mtn_cm` as the key (`SmsProvider::key`, and the path
    /// segment `backends/apps/sms-gateway/src/dlr.rs`'s handler matches
    /// against).
    ///
    /// Example: `https://sms.example.cm/dlr/mtn_cm`.
    #[arg(long)]
    pub(crate) delivery_report_url: String,

    /// MADAPI's own `targetSystem` field — "the name of the system that
    /// this Mobile originating request will be sent to", per the
    /// vendored Swagger's `ShortCodeSubscription`. Free text on MTN's
    /// side; a recognisable name for this deployment is the useful
    /// value.
    #[arg(long, default_value = "vsms")]
    pub(crate) target_system: String,
}

/// Registers `delivery_report_url` with MADAPI and prints the resulting
/// subscription id.
///
/// # Errors
///
/// The token exchange fails, MADAPI refuses the subscription, or the
/// response carries no subscription id — every one of which
/// [`MtnProvider::subscribe_delivery_reports`] already classifies; this
/// function only adds context.
pub(crate) async fn mtn_subscribe_dlr_command(args: MtnSubscribeDlrArgs) -> Result<()> {
    let MtnSubscribeDlrArgs {
        mtn_client_id,
        mtn_client_secret,
        mtn_service_code,
        mtn_base_url,
        delivery_report_url,
        target_system,
    } = args;

    // Every commercial term (`tps_ceiling`, `cost_per_segment_xaf`,
    // `supports_alphanumeric_sender`) is irrelevant to a subscription
    // call — it neither sends a message nor prices one — so this builds
    // a config with placeholder values for them rather than growing three
    // more flags an operator would have to supply for no reason. The
    // values are never read on this path: `subscribe_delivery_reports`
    // touches `client_id`/`client_secret`/`service_code`/`base_url` only.
    let config = MtnConfig::for_subscription_only(
        mtn_client_id,
        mtn_client_secret,
        mtn_service_code,
        mtn_base_url,
    );
    let provider = MtnProvider::new(config);

    let subscription_id = provider
        .subscribe_delivery_reports(&delivery_report_url, &target_system)
        .await
        .context(
            "registering the delivery-report URL with MADAPI — note MTN also requires \
             `requestDeliveryReceipt: true` on each outbound message, which this adapter \
             always sends",
        )?;

    println!("subscription registered: {subscription_id}");
    println!(
        "MTN will now POST delivery receipts to {delivery_report_url} for service code \
         messages sent with requestDeliveryReceipt: true."
    );

    Ok(())
}
