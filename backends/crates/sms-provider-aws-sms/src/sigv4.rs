#![doc = include_str!("sigv4.md")]

use std::fmt::Write as _;

use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

type HmacSha256 = Hmac<Sha256>;

/// The two halves of a long-lived IAM access key. `Debug` is hand-written
/// in the parent module's config type, never derived here, so the secret
/// cannot reach a log line through this struct.
pub(crate) struct Credentials<'a> {
    pub(crate) access_key_id: &'a str,
    pub(crate) secret_access_key: &'a str,
}

/// One request to sign. `headers` are the headers that will actually be
/// sent, apart from `host` and `x-amz-date`, which [`sign`] adds itself
/// because both are derived rather than chosen.
pub(crate) struct Request<'a> {
    pub(crate) method: &'a str,
    /// The URL's authority as it goes on the wire: host, plus `:port` only
    /// when the port is not the scheme's default. It must equal the `Host`
    /// header the HTTP client sends, byte for byte.
    pub(crate) host: &'a str,
    pub(crate) path: &'a str,
    pub(crate) headers: &'a [(&'a str, &'a str)],
    pub(crate) body: &'a [u8],
    pub(crate) region: &'a str,
    pub(crate) service: &'a str,
}

/// The two headers a signed request must carry.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Signature {
    pub(crate) amz_date: String,
    pub(crate) authorization: String,
}

/// Sign `request` as of `now` with AWS Signature Version 4.
///
/// Only what this crate sends is supported: no query string (the JSON
/// protocol puts everything in the body) and a path that needs no
/// URI-encoding. Header names are lowercased and sorted here, and values
/// trimmed, so a caller cannot get the canonical form wrong by passing
/// them in another order.
pub(crate) fn sign(
    credentials: &Credentials<'_>,
    request: &Request<'_>,
    now: DateTime<Utc>,
) -> Signature {
    let amz_date = now.format("%Y%m%dT%H%M%SZ").to_string();
    let date = &amz_date[..8];

    let mut headers: Vec<(String, String)> = request
        .headers
        .iter()
        .map(|(name, value)| (name.to_ascii_lowercase(), value.trim().to_owned()))
        .chain([
            ("host".to_owned(), request.host.to_owned()),
            ("x-amz-date".to_owned(), amz_date.clone()),
        ])
        .collect();
    headers.sort();

    let canonical_headers = headers
        .iter()
        .fold(String::new(), |mut out, (name, value)| {
            let _ = writeln!(out, "{name}:{value}");
            out
        });
    let signed_headers = headers
        .iter()
        .map(|(name, _)| name.as_str())
        .collect::<Vec<_>>()
        .join(";");

    let canonical_request = format!(
        "{method}\n{path}\n\n{canonical_headers}\n{signed_headers}\n{payload}",
        method = request.method,
        path = request.path,
        payload = hex::encode(Sha256::digest(request.body)),
    );

    let scope = format!(
        "{date}/{region}/{service}/aws4_request",
        region = request.region,
        service = request.service
    );
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{}",
        hex::encode(Sha256::digest(canonical_request.as_bytes()))
    );

    let k_date = hmac(
        format!("AWS4{}", credentials.secret_access_key).as_bytes(),
        date.as_bytes(),
    );
    let k_region = hmac(&k_date, request.region.as_bytes());
    let k_service = hmac(&k_region, request.service.as_bytes());
    let k_signing = hmac(&k_service, b"aws4_request");
    let signature = hex::encode(hmac(&k_signing, string_to_sign.as_bytes()));

    Signature {
        authorization: format!(
            "AWS4-HMAC-SHA256 Credential={akid}/{scope}, SignedHeaders={signed_headers}, Signature={signature}",
            akid = credentials.access_key_id,
        ),
        amz_date,
    }
}

fn hmac(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC-SHA256 accepts a key of any length");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

#[cfg(test)]
mod tests {
    use super::{Credentials, Request, Signature, sign};
    use chrono::{TimeZone, Utc};

    /// AWS's own documentation credentials, used by its published test
    /// suite and by every fixture below.
    const CREDENTIALS: Credentials<'static> = Credentials {
        access_key_id: "AKIDEXAMPLE",
        secret_access_key: "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY",
    };

    /// `get-vanilla` from AWS's published `SigV4` test suite. The expected
    /// signature is AWS's own value, and was also reproduced with botocore
    /// before being trusted here (see `sigv4.md`).
    #[test]
    fn reproduces_aws_published_get_vanilla_vector() {
        let signature = sign(
            &CREDENTIALS,
            &Request {
                method: "GET",
                host: "example.amazonaws.com",
                path: "/",
                headers: &[],
                body: b"",
                region: "us-east-1",
                service: "service",
            },
            Utc.with_ymd_and_hms(2015, 8, 30, 12, 36, 0).unwrap(),
        );
        assert_eq!(
            signature,
            Signature {
                amz_date: "20150830T123600Z".to_owned(),
                authorization: "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/service/aws4_request, \
                                SignedHeaders=host;x-amz-date, \
                                Signature=5fa00fa31553b73ebf1942676e86291e8372ff2a2260956d9b8aae1d763fbf31"
                    .to_owned(),
            }
        );
    }

    /// The exact request shape `submit` sends, signed by botocore 1.43.116
    /// for the same credentials, instant, headers and body. botocore is an
    /// independent implementation, so agreement means the canonical form
    /// here is AWS's, not merely self-consistent.
    #[test]
    fn matches_botocore_for_a_send_text_message_request() {
        let body = br#"{"DestinationPhoneNumber":"+237677123456","MessageBody":"Votre code: 123456","MessageType":"TRANSACTIONAL","ConfigurationSetName":"vsms-events","Context":{"vsmsMessageId":"c0abc123"}}"#;
        let signature = sign(
            &CREDENTIALS,
            &Request {
                method: "POST",
                host: "sms-voice.eu-west-1.amazonaws.com",
                path: "/",
                // Deliberately out of order and mixed-case: `sign` must
                // canonicalise, not trust the caller's ordering.
                headers: &[
                    ("X-Amz-Target", "PinpointSMSVoiceV2.SendTextMessage"),
                    ("Content-Type", "application/x-amz-json-1.0"),
                ],
                body,
                region: "eu-west-1",
                service: "sms-voice",
            },
            Utc.with_ymd_and_hms(2026, 10, 21, 12, 0, 0).unwrap(),
        );
        assert_eq!(signature.amz_date, "20261021T120000Z");
        assert_eq!(
            signature.authorization,
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20261021/eu-west-1/sms-voice/aws4_request, \
             SignedHeaders=content-type;host;x-amz-date;x-amz-target, \
             Signature=528f9f0cfae42429de28c9aca752b8c9bf3f569bce4f450572e7c7100eeb2cc1"
        );
    }

    #[test]
    fn a_one_byte_body_change_changes_the_signature() {
        let at = Utc.with_ymd_and_hms(2026, 10, 21, 12, 0, 0).unwrap();
        let request = |body: &'static [u8]| Request {
            method: "POST",
            host: "sms-voice.eu-west-1.amazonaws.com",
            path: "/",
            headers: &[],
            body,
            region: "eu-west-1",
            service: "sms-voice",
        };
        assert_ne!(
            sign(&CREDENTIALS, &request(b"{\"a\":1}"), at),
            sign(&CREDENTIALS, &request(b"{\"a\":2}"), at)
        );
    }
}
