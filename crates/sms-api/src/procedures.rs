//! The seven procedures the schema declares.
//!
//! `previewMessage` is implemented — it is pure, it needs no database, and it
//! is milestone 0's stated gate ("`previewMessage` correct on a French corpus
//! incl. `ç` and `’`"). The other six touch the database, the provider
//! abstraction or the OIDC provider, none of which exist yet; each returns a
//! clearly-labelled error naming the milestone that will build it, rather than
//! a plausible-looking stub that would pass a smoke test and lie.

use cratestack::{CoolContext, CoolError};
use sms_encoding::{analyse, normalise, SmsEncoding};
use sms_msisdn::Msisdn;

use crate::schema;

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

/// Implementations behind the generated router.
#[derive(Debug, Clone, Copy, Default)]
pub struct Procedures;

impl Procedures {
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
            segments: i64::try_from(report.segments).unwrap_or(i64::from(u8::MAX)),
            length: i64::try_from(report.length).unwrap_or(i64::MAX),
            perSegment: i64::try_from(report.per_segment).unwrap_or(i64::MAX),
            offending: distinct_offending(&report),
            suggestion: report.suggestion.clone(),
            // Operator inference is a database lookup against a prefix table
            // that the schema does not yet model (see the note in
            // docs/architecture.md §3.4 — the table must be data, never a
            // compiled-in match). Until that model exists, preview reports
            // `unknown` rather than guessing.
            operator: schema::OperatorCode::unknown,
            normalizedTo: normalized_to,
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
        _db: &schema::Cratestack,
        _ctx: &CoolContext,
        _args: schema::procedures::send_message::Args,
    ) -> impl core::future::Future<
        Output = Result<schema::procedures::send_message::Output, CoolError>,
    > + Send {
        core::future::ready(Err(not_yet(
            "sendMessage",
            "milestone 2 (sms-worker + Orange adapter)",
        )))
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
        _db: &schema::Cratestack,
        _ctx: &CoolContext,
        _args: schema::procedures::provision_app_client::Args,
    ) -> impl core::future::Future<
        Output = Result<schema::procedures::provision_app_client::Output, CoolError>,
    > + Send {
        core::future::ready(Err(not_yet(
            "provisionAppClient",
            "milestone 1 (sms-auth and its custom ClientStore)",
        )))
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
}
