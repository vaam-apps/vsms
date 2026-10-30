//! The fake's keys are generated, distinct and stable for the process, and none
//! is committed.

use std::collections::BTreeSet;

use sms_fake_oidc::{FakeOidc, SigningKey};

async fn jwks(fake: &FakeOidc) -> serde_json::Value {
    reqwest::get(format!("{}/jwks.json", fake.issuer()))
        .await
        .expect("the fake answers")
        .json()
        .await
        .expect("the JWKS is JSON")
}

fn kids(document: &serde_json::Value) -> BTreeSet<String> {
    document["keys"]
        .as_array()
        .expect("keys is an array")
        .iter()
        .map(|key| key["kid"].as_str().expect("every key has a kid").to_owned())
        .collect()
}

#[tokio::test]
async fn the_published_keys_are_the_two_the_fake_starts_with_and_a_rotation_adds_one() {
    let fake = FakeOidc::start("client", "secret").await;
    let before = jwks(&fake).await;
    assert_eq!(
        kids(&before),
        BTreeSet::from(["fake-rsa-1".to_owned(), "fake-ec-1".to_owned()])
    );
    fake.publish_key(SigningKey::Rsa2);
    let after = jwks(&fake).await;
    assert_eq!(
        kids(&after),
        BTreeSet::from([
            "fake-rsa-1".to_owned(),
            "fake-rsa-2".to_owned(),
            "fake-ec-1".to_owned()
        ])
    );
    // Rsa2 publishes no `alg`: a test can sign it with another algorithm.
    let rsa2 = after["keys"]
        .as_array()
        .unwrap()
        .iter()
        .find(|key| key["kid"] == "fake-rsa-2")
        .unwrap();
    assert!(rsa2.get("alg").is_none());
}

#[tokio::test]
async fn each_key_is_its_own_and_a_key_does_not_change_within_the_process() {
    let fake = FakeOidc::start("client", "secret").await;
    fake.publish_key(SigningKey::Rsa2);
    fake.publish_key(SigningKey::Rsa3);
    let first = jwks(&fake).await;
    let moduli: BTreeSet<String> = first["keys"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|key| key["kty"] == "RSA")
        .map(|key| key["n"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(moduli.len(), 3, "three RSA keys, three different moduli");
    // A second fake in the same process publishes the same keys: the "wrong key"
    // tests rely on a kid naming one key for the whole run.
    let other = FakeOidc::start("client", "secret").await;
    other.publish_key(SigningKey::Rsa2);
    other.publish_key(SigningKey::Rsa3);
    assert_eq!(first, jwks(&other).await);
}

/// A committed private key is one a secret scanner cannot tell from a real one,
/// which is why these are generated. This fails the day a PEM literal returns.
#[test]
fn no_private_key_is_committed_in_the_crate() {
    let marker = ["-----BEGIN", " PRIVATE KEY-----"].concat();
    for (file, text) in [
        ("keys.rs", include_str!("../src/keys.rs")),
        ("lib.rs", include_str!("../src/lib.rs")),
    ] {
        assert!(
            !text.contains(&marker),
            "{file} carries a private key literal"
        );
    }
}
