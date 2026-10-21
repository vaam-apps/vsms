#![doc = include_str!("sns.md")]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine as _;
use rustls_pki_types::CertificateDer;
use rustls_pki_types::pem::PemObject as _;
use serde::Deserialize;
use sms_provider::ProviderError;
use url::Url;

/// The only signature version accepted. Version 1 is `SHA1withRSA`; see
/// `sns.md` for why it is refused rather than verified.
const ACCEPTED_SIGNATURE_VERSION: &str = "2";

/// A signing certificate is a small PEM file. Anything larger than this is
/// not one, and is not read into memory.
const MAX_FETCH_BYTES: usize = 64 * 1024;

/// Distinct certificates kept at once. SNS rotates its signing certificate
/// rarely, so a handful covers the overlap of an old and a new one; the
/// cache is cleared rather than grown past this.
const MAX_CACHED_CERTS: usize = 8;

/// One SNS HTTP(S) delivery, as SNS posts it (raw message delivery off).
/// Field names are SNS's own; see `sns.md` for the source.
#[derive(Debug, Deserialize)]
pub(crate) struct Envelope {
    #[serde(rename = "Type")]
    pub(crate) kind: String,
    #[serde(rename = "MessageId")]
    message_id: String,
    #[serde(rename = "Token", default)]
    token: Option<String>,
    #[serde(rename = "TopicArn")]
    pub(crate) topic_arn: String,
    #[serde(rename = "Subject", default)]
    subject: Option<String>,
    #[serde(rename = "Message")]
    pub(crate) message: String,
    #[serde(rename = "SubscribeURL", default)]
    subscribe_url: Option<String>,
    #[serde(rename = "Timestamp")]
    timestamp: String,
    #[serde(rename = "SignatureVersion")]
    signature_version: String,
    #[serde(rename = "Signature")]
    signature: String,
    #[serde(rename = "SigningCertURL")]
    signing_cert_url: String,
}

impl Envelope {
    /// Parse a request body. `Err` carries a human-readable reason.
    pub(crate) fn parse(body: &[u8]) -> Result<Self, String> {
        serde_json::from_slice(body).map_err(|error| format!("not an SNS message: {error}"))
    }
}

/// The exact bytes SNS signed: `name\nvalue\n` pairs in byte order of name,
/// a different field set for notifications and for (un)subscribe
/// confirmations, `Subject` only when present. `None` when a field the
/// type requires is missing, which a genuine SNS message never does.
pub(crate) fn string_to_sign(envelope: &Envelope) -> Option<String> {
    let mut fields: Vec<(&str, &str)> = vec![
        ("Message", &envelope.message),
        ("MessageId", &envelope.message_id),
    ];
    match envelope.kind.as_str() {
        "Notification" => {
            if let Some(subject) = &envelope.subject {
                fields.push(("Subject", subject));
            }
            fields.push(("Timestamp", &envelope.timestamp));
            fields.push(("TopicArn", &envelope.topic_arn));
        }
        "SubscriptionConfirmation" | "UnsubscribeConfirmation" => {
            fields.push(("SubscribeURL", envelope.subscribe_url.as_deref()?));
            fields.push(("Timestamp", &envelope.timestamp));
            fields.push(("Token", envelope.token.as_deref()?));
            fields.push(("TopicArn", &envelope.topic_arn));
        }
        _ => return None,
    }
    fields.push(("Type", &envelope.kind));
    Some(
        fields
            .into_iter()
            .fold(String::new(), |mut out, (name, value)| {
                out.push_str(name);
                out.push('\n');
                out.push_str(value);
                out.push('\n');
                out
            }),
    )
}

/// The region an SNS topic lives in, from its ARN
/// (`arn:aws:sns:<region>:<account>:<name>`). Only the `aws` partition:
/// China and `GovCloud` use other hostnames this module does not accept.
#[must_use]
pub fn region_of_topic_arn(arn: &str) -> Option<&str> {
    let parts: Vec<&str> = arn.split(':').collect();
    match parts.as_slice() {
        ["arn", "aws", "sns", region, account, name]
            if !region.is_empty() && !account.is_empty() && !name.is_empty() =>
        {
            Some(region)
        }
        _ => None,
    }
}

/// `raw`, if and only if it is an `https` URL on exactly
/// `sns.<region>.amazonaws.com`, default port, no credentials. The region
/// is the configured topic's, so a certificate (or confirmation URL) from
/// SNS in some *other* region is refused too.
fn sns_url(raw: &str, region: &str) -> Result<Url, String> {
    let url = Url::parse(raw).map_err(|error| format!("{raw:?} is not a URL: {error}"))?;
    let expected_host = format!("sns.{region}.amazonaws.com");
    if url.scheme() != "https"
        || url.host_str() != Some(expected_host.as_str())
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(format!("{raw:?} is not an https URL on {expected_host}"));
    }
    Ok(url)
}

/// Outbound HTTP the verifier needs: fetching a signing certificate and
/// visiting a `SubscribeURL`. A trait so tests can serve a fixture
/// certificate without a TLS server pretending to be `amazonaws.com`.
#[async_trait]
pub trait SnsHttp: Send + Sync {
    /// GET `url` and return the body. Only ever called with a URL that
    /// already passed [`sns_url`].
    async fn get(&self, url: &Url) -> Result<Vec<u8>, String>;
}

/// [`SnsHttp`] over a real HTTPS client.
pub(crate) struct ReqwestSnsHttp {
    client: reqwest::Client,
}

impl ReqwestSnsHttp {
    pub(crate) fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(5))
                .timeout(Duration::from_secs(10))
                // A redirect would take the request off the host that was
                // just validated.
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .expect("reqwest client builder with timeouts and a redirect policy never fails"),
        }
    }
}

#[async_trait]
impl SnsHttp for ReqwestSnsHttp {
    async fn get(&self, url: &Url) -> Result<Vec<u8>, String> {
        let response = self
            .client
            .get(url.clone())
            .send()
            .await
            .map_err(|error| format!("GET {url} failed: {error}"))?;
        if !response.status().is_success() {
            return Err(format!("GET {url} returned {}", response.status()));
        }
        if response
            .content_length()
            .is_some_and(|len| len > MAX_FETCH_BYTES as u64)
        {
            return Err(format!("GET {url} is larger than {MAX_FETCH_BYTES} bytes"));
        }
        let body = response
            .bytes()
            .await
            .map_err(|error| format!("reading GET {url} failed: {error}"))?;
        if body.len() > MAX_FETCH_BYTES {
            return Err(format!("GET {url} is larger than {MAX_FETCH_BYTES} bytes"));
        }
        Ok(body.to_vec())
    }
}

/// Whether an envelope is authentic.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Verdict {
    Authentic,
    Refused(String),
}

/// Checks SNS envelopes for one configured topic.
pub(crate) struct Verifier {
    topic_arn: String,
    region: String,
    http: Arc<dyn SnsHttp>,
    certs: Mutex<HashMap<String, Arc<CertificateDer<'static>>>>,
}

impl Verifier {
    /// `None` when `topic_arn` is not an `aws`-partition SNS topic ARN.
    pub(crate) fn new(topic_arn: String, http: Arc<dyn SnsHttp>) -> Option<Self> {
        let region = region_of_topic_arn(&topic_arn)?.to_owned();
        Some(Self {
            topic_arn,
            region,
            http,
            certs: Mutex::new(HashMap::new()),
        })
    }

    /// Decide whether `envelope` came from SNS, for the configured topic.
    ///
    /// The order is cheapest first and network last: nothing is fetched
    /// for a message that names the wrong topic or the wrong certificate
    /// host, so a forger cannot make this process issue requests.
    ///
    /// # Errors
    ///
    /// [`ProviderError::Unavailable`] when the signing certificate could not
    /// be fetched — the check did not run, which is not a refusal.
    pub(crate) async fn verify(&self, envelope: &Envelope) -> Result<Verdict, ProviderError> {
        if envelope.topic_arn != self.topic_arn {
            return Ok(Verdict::Refused(format!(
                "TopicArn {:?} is not the configured events topic",
                envelope.topic_arn
            )));
        }
        if envelope.signature_version != ACCEPTED_SIGNATURE_VERSION {
            return Ok(Verdict::Refused(format!(
                "SignatureVersion {:?} refused; set the topic's SignatureVersion attribute to 2",
                envelope.signature_version
            )));
        }
        let cert_url = match sns_url(&envelope.signing_cert_url, &self.region) {
            Ok(url)
                if std::path::Path::new(url.path())
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("pem")) =>
            {
                url
            }
            Ok(url) => {
                return Ok(Verdict::Refused(format!(
                    "SigningCertURL {url} is not a .pem file"
                )));
            }
            Err(reason) => return Ok(Verdict::Refused(format!("SigningCertURL: {reason}"))),
        };
        let Some(signed) = string_to_sign(envelope) else {
            return Ok(Verdict::Refused(format!(
                "a {:?} message without the fields its signature covers",
                envelope.kind
            )));
        };
        let Ok(signature) = base64::engine::general_purpose::STANDARD.decode(&envelope.signature)
        else {
            return Ok(Verdict::Refused("Signature is not base64".to_owned()));
        };

        let cert = self.certificate(&cert_url).await?;
        let Ok(end_entity) = webpki::EndEntityCert::try_from(cert.as_ref()) else {
            return Ok(Verdict::Refused(format!(
                "{cert_url} is not an X.509 certificate"
            )));
        };
        match end_entity.verify_signature(
            webpki::ring::RSA_PKCS1_2048_8192_SHA256,
            signed.as_bytes(),
            &signature,
        ) {
            Ok(()) => Ok(Verdict::Authentic),
            Err(error) => Ok(Verdict::Refused(format!(
                "signature does not verify: {error}"
            ))),
        }
    }

    /// Visit an authenticated `SubscriptionConfirmation`'s `SubscribeURL`,
    /// which is how an HTTPS subscription is confirmed.
    ///
    /// # Errors
    ///
    /// [`ProviderError::Rejected`] for a `SubscribeURL` off the topic's own
    /// SNS host; [`ProviderError::Unavailable`] when visiting it failed.
    pub(crate) async fn confirm_subscription(
        &self,
        envelope: &Envelope,
    ) -> Result<(), ProviderError> {
        let raw = envelope.subscribe_url.as_deref().unwrap_or_default();
        let url = sns_url(raw, &self.region).map_err(|reason| ProviderError::Rejected {
            code: "BAD_SUBSCRIBE_URL".to_owned(),
            message: reason,
        })?;
        self.http
            .get(&url)
            .await
            .map(|_| ())
            .map_err(|message| ProviderError::Unavailable {
                message,
                source: None,
            })
    }

    async fn certificate(&self, url: &Url) -> Result<Arc<CertificateDer<'static>>, ProviderError> {
        if let Some(cert) = self.lock_certs().get(url.as_str()) {
            return Ok(cert.clone());
        }
        let pem = self
            .http
            .get(url)
            .await
            .map_err(|message| ProviderError::Unavailable {
                message,
                source: None,
            })?;
        let cert =
            CertificateDer::from_pem_slice(&pem).map_err(|error| ProviderError::Unavailable {
                message: format!("{url} did not contain a PEM certificate: {error}"),
                source: None,
            })?;
        let cert = Arc::new(cert.into_owned());
        let mut certs = self.lock_certs();
        if certs.len() >= MAX_CACHED_CERTS {
            certs.clear();
        }
        certs.insert(url.as_str().to_owned(), cert.clone());
        Ok(cert)
    }

    fn lock_certs(
        &self,
    ) -> std::sync::MutexGuard<'_, HashMap<String, Arc<CertificateDer<'static>>>> {
        // A panic while holding this lock cannot leave the map half
        // written (every mutation is one insert or one clear), so a
        // poisoned lock is still a usable cache.
        self.certs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::{Envelope, region_of_topic_arn, sns_url, string_to_sign};

    fn envelope(json: &str) -> Envelope {
        Envelope::parse(json.as_bytes()).unwrap()
    }

    /// The field order and the `name\nvalue\n` shape, transcribed from
    /// AWS's own Java example
    /// (<https://docs.aws.amazon.com/sns/latest/dg/sns-example-code-endpoint-java-servlet.html>).
    #[test]
    fn notification_string_to_sign_matches_aws_layout() {
        let env = envelope(
            r#"{"Type":"Notification","MessageId":"m","TopicArn":"arn:aws:sns:eu-west-1:1:t",
                "Message":"body","Timestamp":"ts","SignatureVersion":"2","Signature":"s",
                "SigningCertURL":"u"}"#,
        );
        assert_eq!(
            string_to_sign(&env).unwrap(),
            "Message\nbody\nMessageId\nm\nTimestamp\nts\nTopicArn\narn:aws:sns:eu-west-1:1:t\nType\nNotification\n"
        );
    }

    #[test]
    fn a_present_subject_is_signed_and_a_null_one_is_not() {
        let with = envelope(
            r#"{"Type":"Notification","MessageId":"m","TopicArn":"a","Subject":"hi",
                "Message":"b","Timestamp":"t","SignatureVersion":"2","Signature":"s","SigningCertURL":"u"}"#,
        );
        assert!(
            string_to_sign(&with)
                .unwrap()
                .contains("Subject\nhi\nTimestamp")
        );
        let null = envelope(
            r#"{"Type":"Notification","MessageId":"m","TopicArn":"a","Subject":null,
                "Message":"b","Timestamp":"t","SignatureVersion":"2","Signature":"s","SigningCertURL":"u"}"#,
        );
        assert!(!string_to_sign(&null).unwrap().contains("Subject"));
    }

    #[test]
    fn confirmation_string_to_sign_covers_token_and_subscribe_url() {
        let env = envelope(
            r#"{"Type":"SubscriptionConfirmation","MessageId":"m","Token":"tok","TopicArn":"a",
                "Message":"b","SubscribeURL":"https://x","Timestamp":"t","SignatureVersion":"2",
                "Signature":"s","SigningCertURL":"u"}"#,
        );
        assert_eq!(
            string_to_sign(&env).unwrap(),
            "Message\nb\nMessageId\nm\nSubscribeURL\nhttps://x\nTimestamp\nt\nToken\ntok\nTopicArn\na\nType\nSubscriptionConfirmation\n"
        );
    }

    #[test]
    fn a_confirmation_missing_its_token_has_nothing_to_verify() {
        let env = envelope(
            r#"{"Type":"SubscriptionConfirmation","MessageId":"m","TopicArn":"a","Message":"b",
                "SubscribeURL":"https://x","Timestamp":"t","SignatureVersion":"2","Signature":"s","SigningCertURL":"u"}"#,
        );
        assert_eq!(string_to_sign(&env), None);
    }

    #[test]
    fn topic_arn_region_extraction() {
        assert_eq!(
            region_of_topic_arn("arn:aws:sns:eu-west-1:123456789012:vsms"),
            Some("eu-west-1")
        );
        assert_eq!(region_of_topic_arn("arn:aws-cn:sns:cn-north-1:1:t"), None);
        assert_eq!(region_of_topic_arn("arn:aws:sqs:eu-west-1:1:t"), None);
        assert_eq!(region_of_topic_arn("arn:aws:sns:eu-west-1:1"), None);
        assert_eq!(region_of_topic_arn("not an arn"), None);
    }

    #[test]
    fn only_the_topic_regions_sns_host_is_accepted() {
        let ok = "https://sns.eu-west-1.amazonaws.com/SimpleNotificationService-abc.pem";
        assert!(sns_url(ok, "eu-west-1").is_ok());
        for bad in [
            "http://sns.eu-west-1.amazonaws.com/c.pem",
            "https://sns.us-east-1.amazonaws.com/c.pem",
            "https://sns.eu-west-1.amazonaws.com.evil.example/c.pem",
            "https://evil.example/sns.eu-west-1.amazonaws.com/c.pem",
            "https://user@sns.eu-west-1.amazonaws.com/c.pem",
            "https://sns.eu-west-1.amazonaws.com:8443/c.pem",
            "https://xsns.eu-west-1.amazonaws.com/c.pem",
            "not a url",
        ] {
            assert!(sns_url(bad, "eu-west-1").is_err(), "{bad} must be refused");
        }
    }
}
