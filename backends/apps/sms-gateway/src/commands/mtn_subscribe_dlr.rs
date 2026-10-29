//! `Command::MtnSubscribeDlr` — registers this deployment's DLR endpoint
//! with MADAPI. See that variant's own doc comment in `main.rs` for why
//! this exists as an operator action rather than something `serve` does
//! on startup.

use anyhow::{Context, Result, bail};
use sms_provider_mtn::MtnProvider;

/// `Command::MtnSubscribeDlr`'s flags. The enum variant carries the
/// "why"; this struct carries only the flags themselves, matching
/// `RecordRouteValidationArgs`' own split.
///
/// MTN's credentials are the `providers[code=mtn_cm]` entry of the YAML
/// configuration (VSC-1), the same entry `serve` and `sms-worker` read, so there
/// is one place they are written and this command can no longer be run against
/// a different account than the gateway holds by a typo. It still needs no
/// database, which is why there is no `--database-url` here unlike every other
/// subcommand in this module. It does need the whole document to load: every
/// `${VAR}` it writes must be set, the hash pepper's included, so run it where
/// the gateway's environment is (`kubectl exec`, `docker compose exec`).
#[derive(Debug, clap::Args)]
pub(crate) struct MtnSubscribeDlrArgs {
    /// The YAML deployment configuration, which carries MTN's credentials.
    #[command(flatten)]
    pub(crate) config: sms_config::ConfigArgs,

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
/// The configuration does not load or has no `mtn_cm` entry, the token exchange
/// fails, MADAPI refuses the subscription, or the response carries no
/// subscription id — every one of which
/// [`MtnProvider::subscribe_delivery_reports`] already classifies; this
/// function only adds context.
pub(crate) async fn mtn_subscribe_dlr_command(args: MtnSubscribeDlrArgs) -> Result<()> {
    let MtnSubscribeDlrArgs {
        config,
        delivery_report_url,
        target_system,
    } = args;

    let config = config
        .load(sms_config::Binary::ProviderTool)
        .context("loading the YAML configuration")?;
    let Some(mtn) = config.providers.mtn_cm else {
        bail!(
            "the configuration has no `providers` entry with `code: mtn_cm`, so there are no \
             MTN credentials to register a delivery-report URL with"
        );
    };

    // Every commercial term (`tps_ceiling`, `cost_per_segment_xaf`,
    // `supports_alphanumeric_sender`) is irrelevant to a subscription
    // call — it neither sends a message nor prices one — so the config below
    // carries placeholder values for them (`MtnConfig::for_subscription_only`).
    // The values are never read on this path: `subscribe_delivery_reports`
    // touches `client_id`/`client_secret`/`service_code`/`base_url` only.
    let provider = MtnProvider::new(mtn.to_subscription_config());

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
