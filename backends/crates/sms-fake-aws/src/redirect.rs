//! [`RedirectSnsHttp`]: fetch an SNS signing certificate from the fake.

use std::time::Duration;

use async_trait::async_trait;
use sms_provider_aws_sms::SnsHttp;
use url::Url;

/// A signing certificate is a small PEM file; read no more than this.
const MAX_FETCH_BYTES: usize = 64 * 1024;

/// An [`SnsHttp`] that fetches every signing certificate from `base`
/// instead of AWS, keeping the path.
///
/// The receiver still refuses any `SigningCertURL` that is not
/// `https://sns.<topic region>.amazonaws.com/SimpleNotificationService-<32 hex>.pem`
/// before this is ever called; only where the bytes come from changes. So a
/// receiver built with this trusts exactly one thing it otherwise would
/// not: a certificate the fake made. That is a trust decision, which is why
/// `sms-gateway` reaches this only through its off-by-default `fake-aws`
/// feature.
pub struct RedirectSnsHttp {
    base: Url,
    client: reqwest::Client,
}

impl RedirectSnsHttp {
    /// # Errors
    ///
    /// When `base` is not an absolute `http(s)` URL without a query.
    ///
    /// # Panics
    ///
    /// Never in practice: building a `reqwest` client with only timeouts
    /// and a redirect policy cannot fail.
    pub fn new(base: &str) -> Result<Self, String> {
        let mut base = Url::parse(base).map_err(|error| format!("{base:?}: {error}"))?;
        if !matches!(base.scheme(), "http" | "https")
            || base.query().is_some()
            || base.host_str().is_none()
        {
            return Err(format!("{base} is not an http(s) base URL"));
        }
        if !base.path().ends_with('/') {
            let path = format!("{}/", base.path());
            base.set_path(&path);
        }
        Ok(Self {
            base,
            client: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(5))
                .timeout(Duration::from_secs(10))
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .expect("reqwest client builder with timeouts never fails"),
        })
    }

    /// Where a certificate URL is actually fetched from.
    fn target(&self, url: &Url) -> Result<Url, String> {
        self.base
            .join(url.path().trim_start_matches('/'))
            .map_err(|error| format!("redirecting {url}: {error}"))
    }
}

#[async_trait]
impl SnsHttp for RedirectSnsHttp {
    async fn get(&self, url: &Url) -> Result<Vec<u8>, String> {
        let target = self.target(url)?;
        let response = self
            .client
            .get(target.clone())
            .send()
            .await
            .map_err(|error| format!("GET {target} (for {url}) failed: {error}"))?;
        if !response.status().is_success() {
            return Err(format!("GET {target} returned {}", response.status()));
        }
        let body = response
            .bytes()
            .await
            .map_err(|error| format!("reading GET {target} failed: {error}"))?;
        if body.len() > MAX_FETCH_BYTES {
            return Err(format!(
                "GET {target} is larger than {MAX_FETCH_BYTES} bytes"
            ));
        }
        Ok(body.to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::RedirectSnsHttp;
    use url::Url;

    #[test]
    fn only_the_path_survives_the_redirect() {
        let http = RedirectSnsHttp::new("http://sms-fake-aws:8095").unwrap();
        let cert = Url::parse(
            "https://sns.eu-west-1.amazonaws.com/SimpleNotificationService-0123456789abcdef0123456789abcdef.pem",
        )
        .unwrap();
        assert_eq!(
            http.target(&cert).unwrap().as_str(),
            "http://sms-fake-aws:8095/SimpleNotificationService-0123456789abcdef0123456789abcdef.pem"
        );
    }

    #[test]
    fn a_base_that_is_not_http_is_refused() {
        assert!(RedirectSnsHttp::new("file:///etc").is_err());
        assert!(RedirectSnsHttp::new("http://x/?q=1").is_err());
        assert!(RedirectSnsHttp::new("not a url").is_err());
    }
}
