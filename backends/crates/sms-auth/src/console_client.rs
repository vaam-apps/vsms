#![doc = include_str!("console_client.md")]

use cratestack::{CratestackContext, CratestackError, FilterExpr};
use sms_api::schema::{
    self, ClientAuthMethod, Cratestack, CreateOauthClientInput, UpdateOauthClientInput,
    oauth_client,
};
use sms_config::ConsoleClient;
use sms_core::{PackError, pack, unpack};
use thiserror::Error;

use crate::op::{AUTHORIZATION_CODE, CONSOLE_SCOPES};

/// The command an operator runs to make the row match the configuration. Named
/// in every error that says it does not.
pub const SEED_COMMAND: &str = "sms-gateway seed-console-client";

/// One way the stored console client differs from what the configuration (and
/// this release's rules) say it must be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mismatch {
    /// `OauthClient.active` is false: the provider would not find it.
    Inactive,
    /// The client authenticates at `/token` with something other than nothing.
    /// The console is a public client protected by PKCE; a `private_key_jwt` row
    /// of this id would be a machine's.
    AuthMethod,
    /// A public key is registered. A public client has none.
    Jwks,
    /// The row is linked to an `AppClient`: it would then be a machine client.
    AppClient,
    /// `refresh_token` is among the grants. This deployment issues no refresh
    /// token, ever.
    RefreshToken,
    /// The grants are not exactly `authorization_code`. Named with what is stored.
    Grants(Vec<String>),
    /// The scopes are not exactly `openid profile`. Named with what is stored.
    Scopes(Vec<String>),
    /// The redirect URIs are not exactly the configured list. Named with what is
    /// stored.
    RedirectUris(Vec<String>),
    /// `requirePkce` is false.
    Pkce,
}

impl std::fmt::Display for Mismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Inactive => f.write_str("it is inactive"),
            Self::AuthMethod => f.write_str("its token endpoint auth method is not `none`"),
            Self::Jwks => f.write_str("it holds a public key (a public client has none)"),
            Self::AppClient => {
                f.write_str("it is linked to an AppClient (that makes it a machine client)")
            }
            Self::RefreshToken => f.write_str(
                "it carries the refresh_token grant (this deployment issues no refresh token)",
            ),
            Self::Grants(got) => write!(f, "its grants are {got:?}, not [\"authorization_code\"]"),
            Self::Scopes(got) => write!(f, "its scopes are {got:?}, not [\"openid\", \"profile\"]"),
            Self::RedirectUris(got) => {
                write!(f, "its redirect URIs are {got:?}, not the configured list")
            }
            Self::Pkce => f.write_str("it does not require PKCE"),
        }
    }
}

/// Why the console client row is not usable.
#[derive(Debug, Error)]
pub enum ConsoleClientError {
    /// No row has the configured `client_id`.
    #[error(
        "the console client {client_id:?} is not registered: run `{SEED_COMMAND}` (the row it \
         reads is created from human_login.console_client)"
    )]
    Missing {
        /// The configured id.
        client_id: String,
    },
    /// A row has the id and differs from the configuration or from this release's
    /// rules.
    #[error(
        "the console client {client_id:?} does not match the configuration ({}): run \
         `{SEED_COMMAND}` to reconcile it",
        join(.mismatches)
    )]
    Mismatched {
        /// The configured id.
        client_id: String,
        /// Every difference found, not only the first.
        mismatches: Vec<Mismatch>,
    },
    /// The row with the configured id is a **machine client's**: it holds a
    /// public key, is linked to an `AppClient`, or authenticates with
    /// `private_key_jwt`. Neither the boot check nor [`seed_console_client`]
    /// touches it: reconciling would strip a service's credentials and turn it
    /// into the console, and the command's repair is for a stale *console* row.
    #[error(
        "the OauthClient {client_id:?} is a machine client, not a console row ({}): refusing to \
         take it over. Choose another human_login.console_client.client_id, or remove that \
         client deliberately first",
        join(.conflicts)
    )]
    MachineClient {
        /// The configured id.
        client_id: String,
        /// Which of the three machine markers the row carries.
        conflicts: Vec<Mismatch>,
    },
    /// The configured values cannot be stored (a redirect URI that contains the
    /// column's delimiter; the loader refuses these first).
    #[error("human_login.console_client cannot be stored: {0}")]
    Unstorable(#[from] PackError),
    /// A delegate call failed.
    #[error("database: {0}")]
    Database(#[from] CratestackError),
}

fn join(mismatches: &[Mismatch]) -> String {
    mismatches
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("; ")
}

/// Every difference between `row` and what `console` requires. Empty is a
/// match. A pure function, so every rule has a unit test and the live suites only
/// have to prove the wiring.
#[must_use]
pub fn mismatches(row: &schema::OauthClient, console: &ConsoleClient) -> Vec<Mismatch> {
    let mut found = Vec::new();
    if !row.active {
        found.push(Mismatch::Inactive);
    }
    if row.tokenEndpointAuthMethod != ClientAuthMethod::none {
        found.push(Mismatch::AuthMethod);
    }
    if row.jwks.is_some() {
        found.push(Mismatch::Jwks);
    }
    if row.appClientId.is_some() {
        found.push(Mismatch::AppClient);
    }
    let grants: Vec<String> = unpack(&row.grantTypes)
        .into_iter()
        .map(str::to_owned)
        .collect();
    if grants.iter().any(|grant| grant == "refresh_token") {
        found.push(Mismatch::RefreshToken);
    }
    if grants != [AUTHORIZATION_CODE] {
        found.push(Mismatch::Grants(grants));
    }
    let scopes: Vec<String> = unpack(&row.scopes).into_iter().map(str::to_owned).collect();
    if !same_set(&scopes, &CONSOLE_SCOPES.map(str::to_owned)) {
        found.push(Mismatch::Scopes(scopes));
    }
    let redirects: Vec<String> = unpack(&row.redirectUris)
        .into_iter()
        .map(str::to_owned)
        .collect();
    if !same_set(&redirects, &console.redirect_uris) {
        found.push(Mismatch::RedirectUris(redirects));
    }
    if !row.requirePkce {
        found.push(Mismatch::Pkce);
    }
    found
}

/// The differences that mean the row is not a console row at all but a machine
/// client's: a key, an `AppClient`, or an assertion-based auth method. Every other
/// [`Mismatch`] is drift of a console row, which `seed-console-client` repairs.
#[must_use]
pub fn machine_conflicts(differences: &[Mismatch]) -> Vec<Mismatch> {
    differences
        .iter()
        .filter(|difference| {
            matches!(
                difference,
                Mismatch::AuthMethod | Mismatch::Jwks | Mismatch::AppClient
            )
        })
        .cloned()
        .collect()
}

fn same_set(a: &[String], b: &[String]) -> bool {
    let mut a = a.to_vec();
    let mut b = b.to_vec();
    a.sort();
    b.sort();
    a == b
}

async fn find(
    db: &Cratestack,
    sys: &CratestackContext,
    client_id: &str,
) -> Result<Option<schema::OauthClient>, CratestackError> {
    // Not `active`-filtered: an inactive row is a mismatch to name, not a row to
    // pretend is missing.
    Ok(db
        .oauth_client()
        .find_many()
        .where_expr(FilterExpr::from(
            oauth_client::clientId().eq(client_id.to_owned()),
        ))
        .limit(1)
        .run(sys)
        .await?
        .into_iter()
        .next())
}

/// `serve`'s boot check: the console client row exists and matches
/// `human_login.console_client` and this release's rules. Reads uncached, once.
///
/// # Errors
///
/// [`ConsoleClientError::Missing`] or [`ConsoleClientError::Mismatched`], both of
/// which tell the operator to run [`SEED_COMMAND`]; or
/// [`ConsoleClientError::MachineClient`], which does not (the command would refuse
/// it too) and says to choose another id.
pub async fn check_console_client(
    db: &Cratestack,
    sys: &CratestackContext,
    console: &ConsoleClient,
) -> Result<(), ConsoleClientError> {
    let Some(row) = find(db, sys, &console.client_id).await? else {
        return Err(ConsoleClientError::Missing {
            client_id: console.client_id.clone(),
        });
    };
    let differences = mismatches(&row, console);
    if differences.is_empty() {
        return Ok(());
    }
    let conflicts = machine_conflicts(&differences);
    if conflicts.is_empty() {
        Err(ConsoleClientError::Mismatched {
            client_id: console.client_id.clone(),
            mismatches: differences,
        })
    } else {
        Err(ConsoleClientError::MachineClient {
            client_id: console.client_id.clone(),
            conflicts,
        })
    }
}

/// What [`seed_console_client`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Seeded {
    /// No row had the id; one was created.
    Created,
    /// The row differed and was brought into line.
    Reconciled(Vec<Mismatch>),
    /// The row already matched; nothing was written.
    Unchanged,
}

/// Makes the `OauthClient` row for `human_login.console_client` exist and match:
/// created when missing, **reconciled** when it differs (so a changed redirect URI
/// in the file reaches the row, and a leftover `refresh_token` grant is removed),
/// untouched when it already matches. Idempotent.
///
/// **A row that is a machine client's is refused, never reconciled**
/// ([`ConsoleClientError::MachineClient`]): a row with an `AppClient` link, a
/// public key or `private_key_jwt` belongs to a service, and rewriting it into the
/// console would silently cut that service off.
///
/// `sys` is the internal `system` context, the only one `OauthClient` admits.
///
/// # Errors
///
/// [`ConsoleClientError`]; [`ConsoleClientError::MachineClient`] when the row
/// with the configured id is a machine client's.
pub async fn seed_console_client(
    db: &Cratestack,
    sys: &CratestackContext,
    console: &ConsoleClient,
) -> Result<Seeded, ConsoleClientError> {
    let grant_types = pack([AUTHORIZATION_CODE])?;
    let scopes = pack(CONSOLE_SCOPES)?;
    let redirect_uris = pack(&console.redirect_uris)?;

    let Some(row) = find(db, sys, &console.client_id).await? else {
        db.oauth_client()
            .create(CreateOauthClientInput {
                clientId: console.client_id.clone(),
                appClientId: None,
                tokenEndpointAuthMethod: ClientAuthMethod::none,
                jwks: None,
                grantTypes: grant_types,
                scopes,
                redirectUris: redirect_uris,
                requirePkce: true,
            })
            .run(sys)
            .await?;
        return Ok(Seeded::Created);
    };

    let differences = mismatches(&row, console);
    if differences.is_empty() {
        return Ok(Seeded::Unchanged);
    }
    let conflicts = machine_conflicts(&differences);
    if !conflicts.is_empty() {
        return Err(ConsoleClientError::MachineClient {
            client_id: console.client_id.clone(),
            conflicts,
        });
    }
    db.oauth_client()
        .update(row.id)
        .set(UpdateOauthClientInput {
            appClientId: Some(None),
            tokenEndpointAuthMethod: Some(ClientAuthMethod::none),
            jwks: Some(None),
            grantTypes: Some(grant_types),
            scopes: Some(scopes),
            redirectUris: Some(redirect_uris),
            requirePkce: Some(true),
            active: Some(true),
            ..Default::default()
        })
        .run(sys)
        .await?;
    Ok(Seeded::Reconciled(differences))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn console() -> ConsoleClient {
        ConsoleClient {
            client_id: "sms-console".to_owned(),
            redirect_uris: vec![
                "https://admin.example/cb".to_owned(),
                "https://admin.example/cb2".to_owned(),
            ],
        }
    }

    /// A row that matches `console()` exactly: every test breaks one field of it.
    fn row() -> schema::OauthClient {
        schema::OauthClient {
            id: "c00000000000000000000001".to_owned(),
            createdAt: chrono::Utc::now(),
            updatedAt: chrono::Utc::now(),
            clientId: "sms-console".to_owned(),
            appClientId: None,
            tokenEndpointAuthMethod: ClientAuthMethod::none,
            jwks: None,
            grantTypes: " authorization_code ".to_owned(),
            scopes: " openid profile ".to_owned(),
            redirectUris: " https://admin.example/cb2 https://admin.example/cb ".to_owned(),
            requirePkce: true,
            active: true,
        }
    }

    #[test]
    fn a_matching_row_has_no_mismatch_and_order_does_not_matter() {
        assert_eq!(mismatches(&row(), &console()), []);
    }

    #[test]
    fn each_rule_is_named_on_its_own() {
        let cases: Vec<(schema::OauthClient, Mismatch)> = vec![
            (
                schema::OauthClient {
                    active: false,
                    ..row()
                },
                Mismatch::Inactive,
            ),
            (
                schema::OauthClient {
                    tokenEndpointAuthMethod: ClientAuthMethod::private_key_jwt,
                    ..row()
                },
                Mismatch::AuthMethod,
            ),
            (
                schema::OauthClient {
                    jwks: Some("{}".to_owned()),
                    ..row()
                },
                Mismatch::Jwks,
            ),
            (
                schema::OauthClient {
                    appClientId: Some("c00000000000000000000002".to_owned()),
                    ..row()
                },
                Mismatch::AppClient,
            ),
            (
                schema::OauthClient {
                    requirePkce: false,
                    ..row()
                },
                Mismatch::Pkce,
            ),
        ];
        for (broken, expected) in cases {
            assert_eq!(
                mismatches(&broken, &console()),
                std::slice::from_ref(&expected),
                "{expected}"
            );
        }
    }

    #[test]
    fn a_refresh_grant_is_named_as_such_and_as_a_grant_mismatch() {
        let broken = schema::OauthClient {
            grantTypes: " authorization_code refresh_token ".to_owned(),
            ..row()
        };
        let found = mismatches(&broken, &console());
        assert!(found.contains(&Mismatch::RefreshToken), "{found:?}");
        assert!(
            found.iter().any(|m| matches!(m, Mismatch::Grants(_))),
            "{found:?}"
        );
    }

    #[test]
    fn another_grant_alongside_the_code_is_a_mismatch() {
        let broken = schema::OauthClient {
            grantTypes: " authorization_code client_credentials ".to_owned(),
            ..row()
        };
        assert!(matches!(
            mismatches(&broken, &console()).as_slice(),
            [Mismatch::Grants(_)]
        ));
    }

    #[test]
    fn scopes_and_redirects_are_compared_as_sets_to_the_configuration() {
        let wider = schema::OauthClient {
            scopes: " openid profile offline_access ".to_owned(),
            ..row()
        };
        assert!(matches!(
            mismatches(&wider, &console()).as_slice(),
            [Mismatch::Scopes(_)]
        ));
        let stale = schema::OauthClient {
            redirectUris: " https://old.example/cb ".to_owned(),
            ..row()
        };
        assert!(matches!(
            mismatches(&stale, &console()).as_slice(),
            [Mismatch::RedirectUris(_)]
        ));
    }

    #[test]
    fn the_error_names_every_difference_and_the_command() {
        let broken = schema::OauthClient {
            active: false,
            requirePkce: false,
            ..row()
        };
        let error = ConsoleClientError::Mismatched {
            client_id: "sms-console".to_owned(),
            mismatches: mismatches(&broken, &console()),
        };
        let text = error.to_string();
        assert!(text.contains("inactive") && text.contains("PKCE"), "{text}");
        assert!(text.contains(SEED_COMMAND), "{text}");
        assert!(
            ConsoleClientError::Missing {
                client_id: "x".to_owned()
            }
            .to_string()
            .contains(SEED_COMMAND)
        );
    }

    /// Only the three markers of a machine client are conflicts. Drift of a
    /// console row (a redirect URI, a scope, a leftover grant, PKCE off, inactive)
    /// is the repair `seed-console-client` exists for.
    #[test]
    fn only_a_key_an_app_client_or_an_assertion_method_marks_a_machine_row() {
        let machine = schema::OauthClient {
            tokenEndpointAuthMethod: ClientAuthMethod::private_key_jwt,
            jwks: Some("{}".to_owned()),
            appClientId: Some("c00000000000000000000002".to_owned()),
            grantTypes: " client_credentials ".to_owned(),
            ..row()
        };
        let found = machine_conflicts(&mismatches(&machine, &console()));
        assert_eq!(
            found,
            [Mismatch::AuthMethod, Mismatch::Jwks, Mismatch::AppClient]
        );

        let stale_console = schema::OauthClient {
            active: false,
            requirePkce: false,
            grantTypes: " authorization_code refresh_token ".to_owned(),
            scopes: " openid ".to_owned(),
            redirectUris: " https://old.example/cb ".to_owned(),
            ..row()
        };
        assert_eq!(
            machine_conflicts(&mismatches(&stale_console, &console())),
            [],
            "a console row that drifted is repaired, not refused"
        );
    }

    #[test]
    fn the_machine_client_error_names_the_conflict_and_does_not_offer_the_command() {
        let error = ConsoleClientError::MachineClient {
            client_id: "sms-console".to_owned(),
            conflicts: vec![Mismatch::AppClient, Mismatch::Jwks],
        };
        let message = error.to_string();
        assert!(message.contains("machine client"), "{message}");
        assert!(message.contains("AppClient"), "{message}");
        assert!(message.contains("public key"), "{message}");
        assert!(
            message.contains("another human_login.console_client.client_id"),
            "{message}"
        );
        assert!(
            !message.contains(SEED_COMMAND),
            "the command would refuse this row too: {message}"
        );
    }
}
