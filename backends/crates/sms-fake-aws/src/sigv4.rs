#![doc = include_str!("sigv4.md")]

use axum::http::HeaderMap;
use chrono::{DateTime, NaiveDateTime, Utc};
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

type HmacSha256 = Hmac<Sha256>;

/// How far `x-amz-date` may be from the fake's clock. AWS allows 15
/// minutes before answering `Signature expired`.
const MAX_SKEW: chrono::TimeDelta = chrono::TimeDelta::minutes(15);

/// Why a request's signature was refused, as the error type AWS answers
/// with (all `403`).
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Refusal {
    pub(crate) error_type: &'static str,
    pub(crate) message: String,
}

fn refuse(error_type: &'static str, message: impl Into<String>) -> Refusal {
    Refusal {
        error_type,
        message: message.into(),
    }
}

/// The account the fake recognises.
pub(crate) struct Expected<'a> {
    pub(crate) access_key_id: &'a str,
    pub(crate) secret_access_key: &'a str,
    pub(crate) region: &'a str,
    pub(crate) service: &'a str,
}

/// Check a request's AWS Signature Version 4 against the bytes and headers
/// that actually arrived.
pub(crate) fn verify(
    expected: &Expected<'_>,
    method: &str,
    path: &str,
    query: Option<&str>,
    headers: &HeaderMap,
    body: &[u8],
    now: DateTime<Utc>,
) -> Result<(), Refusal> {
    let authorization = header(headers, "authorization").ok_or_else(|| {
        refuse(
            "MissingAuthenticationTokenException",
            "Missing Authentication Token",
        )
    })?;
    let fields = authorization
        .strip_prefix("AWS4-HMAC-SHA256 ")
        .ok_or_else(|| refuse("IncompleteSignatureException", "not AWS4-HMAC-SHA256"))?;
    let mut credential = None;
    let mut signed_headers = None;
    let mut signature = None;
    for part in fields.split(',').map(str::trim) {
        match part.split_once('=') {
            Some(("Credential", value)) => credential = Some(value),
            Some(("SignedHeaders", value)) => signed_headers = Some(value),
            Some(("Signature", value)) => signature = Some(value),
            _ => return Err(refuse("IncompleteSignatureException", format!("{part:?}"))),
        }
    }
    let (Some(credential), Some(signed_headers), Some(signature)) =
        (credential, signed_headers, signature)
    else {
        return Err(refuse(
            "IncompleteSignatureException",
            "Authorization needs Credential, SignedHeaders and Signature",
        ));
    };

    let scope: Vec<&str> = credential.split('/').collect();
    let [access_key_id, date, region, service, terminator] = scope.as_slice() else {
        return Err(refuse(
            "IncompleteSignatureException",
            "malformed Credential",
        ));
    };
    if *access_key_id != expected.access_key_id {
        return Err(refuse(
            "UnrecognizedClientException",
            "The security token included in the request is invalid",
        ));
    }
    if *region != expected.region || *service != expected.service || *terminator != "aws4_request" {
        return Err(refuse(
            "InvalidSignatureException",
            format!(
                "Credential should be scoped to {}/{}/aws4_request, not {region}/{service}/{terminator}",
                expected.region, expected.service
            ),
        ));
    }

    let amz_date = header(headers, "x-amz-date")
        .ok_or_else(|| refuse("IncompleteSignatureException", "missing X-Amz-Date"))?;
    let signed_at = NaiveDateTime::parse_from_str(amz_date, "%Y%m%dT%H%M%SZ")
        .map_err(|_| refuse("IncompleteSignatureException", "malformed X-Amz-Date"))?
        .and_utc();
    if !amz_date.starts_with(date) {
        return Err(refuse(
            "InvalidSignatureException",
            "Credential date does not match X-Amz-Date",
        ));
    }
    if (now - signed_at).abs() > MAX_SKEW {
        return Err(refuse("InvalidSignatureException", "Signature expired"));
    }

    let names: Vec<&str> = signed_headers.split(';').collect();
    let mut sorted = names.clone();
    sorted.sort_unstable();
    sorted.dedup();
    if names != sorted || names.iter().any(|name| name.to_ascii_lowercase() != *name) {
        return Err(refuse(
            "IncompleteSignatureException",
            "SignedHeaders must be lowercase, sorted and unique",
        ));
    }
    for required in ["host", "x-amz-date", "x-amz-target"] {
        if !names.contains(&required) {
            return Err(refuse(
                "IncompleteSignatureException",
                format!("{required} must be signed"),
            ));
        }
    }
    let canonical = canonical_request(method, path, query, headers, &names, body)?;
    let expected_signature = signature_for(
        expected.secret_access_key,
        amz_date,
        &[date, region, service],
        &canonical,
    );
    if expected_signature != signature {
        return Err(refuse(
            "InvalidSignatureException",
            "The request signature we calculated does not match the signature you provided",
        ));
    }
    Ok(())
}

/// The canonical request over what arrived: each signed header's received
/// value, the received body's hash.
fn canonical_request(
    method: &str,
    path: &str,
    query: Option<&str>,
    headers: &HeaderMap,
    signed: &[&str],
    body: &[u8],
) -> Result<String, Refusal> {
    let mut canonical_headers = String::new();
    for name in signed {
        let value = header(headers, name).ok_or_else(|| {
            refuse(
                "IncompleteSignatureException",
                format!("signed header {name} was not sent"),
            )
        })?;
        canonical_headers.push_str(name);
        canonical_headers.push(':');
        canonical_headers.push_str(&value.split_whitespace().collect::<Vec<_>>().join(" "));
        canonical_headers.push('\n');
    }
    Ok(format!(
        "{method}\n{path}\n{query}\n{canonical_headers}\n{signed}\n{payload}",
        query = query.unwrap_or_default(),
        signed = signed.join(";"),
        payload = hex::encode(Sha256::digest(body)),
    ))
}

/// The hex signature of `canonical` for the scope `[date, region, service]`.
fn signature_for(secret: &str, amz_date: &str, scope: &[&str; 3], canonical: &str) -> String {
    let [date, region, service] = scope;
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{amz_date}\n{date}/{region}/{service}/aws4_request\n{}",
        hex::encode(Sha256::digest(canonical.as_bytes()))
    );
    let mut key = format!("AWS4{secret}").into_bytes();
    for part in [*date, *region, *service, "aws4_request"] {
        key = hmac(&key, part.as_bytes());
    }
    hex::encode(hmac(&key, string_to_sign.as_bytes()))
}

fn header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|value| value.to_str().ok())
}

fn hmac(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts keys of any length");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

#[cfg(test)]
mod tests {
    use axum::http::{HeaderMap, HeaderValue};
    use chrono::{TimeZone, Utc};

    use super::{Expected, canonical_request, signature_for, verify};

    const EXPECTED: Expected<'static> = Expected {
        access_key_id: "AKIDEXAMPLE",
        secret_access_key: "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY",
        region: "us-east-1",
        service: "service",
    };

    fn vanilla_headers() -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert("host", HeaderValue::from_static("example.amazonaws.com"));
        headers.insert("x-amz-date", HeaderValue::from_static("20150830T123600Z"));
        headers
    }

    /// AWS's published `get-vanilla` vector from the Signature Version 4 test suite, run
    /// through the same two functions `verify` uses. This is what makes the
    /// fake's check independent of the adapter's signer: both are held to
    /// AWS's own answer, not to each other.
    #[test]
    fn the_aws_get_vanilla_vector_verifies() {
        let canonical = canonical_request(
            "GET",
            "/",
            None,
            &vanilla_headers(),
            &["host", "x-amz-date"],
            b"",
        )
        .unwrap();
        assert_eq!(
            signature_for(
                EXPECTED.secret_access_key,
                "20150830T123600Z",
                &["20150830", "us-east-1", "service"],
                &canonical
            ),
            "5fa00fa31553b73ebf1942676e86291e8372ff2a2260956d9b8aae1d763fbf31"
        );
    }

    fn signed(signed_headers: &str, signature: &str) -> HeaderMap {
        let mut headers = vanilla_headers();
        headers.insert("x-amz-target", HeaderValue::from_static("Svc.Op"));
        headers.insert(
            "authorization",
            HeaderValue::from_str(&format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/service/aws4_request, \
                 SignedHeaders={signed_headers}, Signature={signature}"
            ))
            .unwrap(),
        );
        headers
    }

    #[test]
    fn a_signature_over_different_bytes_is_refused() {
        let now = Utc.with_ymd_and_hms(2015, 8, 30, 12, 36, 0).unwrap();
        let headers = signed("host;x-amz-date;x-amz-target", "00");
        let refused = verify(&EXPECTED, "GET", "/", None, &headers, b"", now).unwrap_err();
        assert_eq!(refused.error_type, "InvalidSignatureException");
    }

    /// The vanilla signature is genuine for host and x-amz-date, but a
    /// signature that leaves the operation unsigned lets the request be
    /// replayed as another operation, so the fake refuses it.
    #[test]
    fn an_unsigned_target_is_refused() {
        let now = Utc.with_ymd_and_hms(2015, 8, 30, 12, 36, 0).unwrap();
        let headers = signed(
            "host;x-amz-date",
            "5fa00fa31553b73ebf1942676e86291e8372ff2a2260956d9b8aae1d763fbf31",
        );
        let refused = verify(&EXPECTED, "GET", "/", None, &headers, b"", now).unwrap_err();
        assert!(refused.message.contains("x-amz-target"), "{refused:?}");
    }

    #[test]
    fn a_stale_date_is_refused() {
        let now = Utc.with_ymd_and_hms(2015, 8, 30, 13, 0, 0).unwrap();
        let headers = signed("host;x-amz-date;x-amz-target", "00");
        let refused = verify(&EXPECTED, "GET", "/", None, &headers, b"", now).unwrap_err();
        assert_eq!(refused.message, "Signature expired");
    }

    #[test]
    fn another_access_key_is_unrecognised() {
        let now = Utc.with_ymd_and_hms(2015, 8, 30, 12, 36, 0).unwrap();
        let mut headers = signed("host;x-amz-date;x-amz-target", "00");
        headers.insert(
            "authorization",
            HeaderValue::from_static(
                "AWS4-HMAC-SHA256 Credential=AKIDOTHER/20150830/us-east-1/service/aws4_request, \
                 SignedHeaders=host;x-amz-date;x-amz-target, Signature=00",
            ),
        );
        let refused = verify(&EXPECTED, "GET", "/", None, &headers, b"", now).unwrap_err();
        assert_eq!(refused.error_type, "UnrecognizedClientException");
    }
}
