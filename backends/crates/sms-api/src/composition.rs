#![doc = include_str!("composition.md")]

use cratestack::{CratestackContext, CratestackError, Value};

use crate::auth::PrincipalKind;
use crate::rbac::require_permission;
use crate::schema;

/// The permission `authorizeComposition` asks of the person (Layer 2). The
/// same literal `sendMessage` asks of an app.
pub const COMPOSE_PERMISSION: &str = "sms:send";

/// The two operations the composer has.
pub const OPERATIONS: [&str; 2] = ["send", "preview"];

fn validation(message: &str) -> CratestackError {
    CratestackError::Validation(message.to_owned())
}

/// A SHA-256 digest as the console spells it: 64 lowercase hex digits.
///
/// ```
/// use sms_api::composition::is_sha256_hex;
///
/// assert!(is_sha256_hex(&"a".repeat(64)));
/// assert!(!is_sha256_hex(&"A".repeat(64)));
/// assert!(!is_sha256_hex(&"a".repeat(63)));
/// assert!(!is_sha256_hex(&format!("{}g", "a".repeat(63))));
/// ```
#[must_use]
pub fn is_sha256_hex(text: &str) -> bool {
    text.len() == 64
        && text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// The answer for a person who may compose, or the refusal.
///
/// # Errors
///
/// [`CratestackError::Forbidden`] for anything but a `user` principal holding
/// [`COMPOSE_PERMISSION`], [`CratestackError::Validation`] for an operation the
/// composer does not have or a hash that is not SHA-256 in lowercase hex.
pub fn authorize(
    ctx: &CratestackContext,
    args: schema::AuthorizeCompositionInput,
) -> Result<schema::CompositionAuthorization, CratestackError> {
    let field = |name: &str| match ctx.auth_field(name) {
        Some(Value::String(value)) => Some(value.clone()),
        _ => None,
    };
    if field("kind").as_deref() != Some(PrincipalKind::User.as_str()) {
        return Err(CratestackError::Forbidden(
            "authorizeComposition is for a signed-in person only".to_owned(),
        ));
    }
    require_permission(ctx, COMPOSE_PERMISSION)?;
    if !OPERATIONS.contains(&args.operation.as_str()) {
        return Err(validation("operation must be `send` or `preview`"));
    }
    if !is_sha256_hex(&args.inputSha256) {
        return Err(validation(
            "inputSha256 must be 64 lowercase hexadecimal digits",
        ));
    }
    let (Some(user_id), Some(role)) = (field("sub"), field("role")) else {
        return Err(CratestackError::Unauthorized(
            "the caller names no person".to_owned(),
        ));
    };
    Ok(schema::CompositionAuthorization {
        userId: user_id,
        role,
        operation: args.operation,
        inputSha256: args.inputSha256,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{GatewayAuth, Principal};
    use crate::signers::{Standing, UserStanding};

    fn person(role: &str, perms: &[&str]) -> CratestackContext {
        GatewayAuth::context_for(Standing::User(UserStanding {
            user_id: "u1".to_owned(),
            role: role.to_owned(),
            perms: perms.iter().map(|p| (*p).to_owned()).collect(),
        }))
    }

    fn args(operation: &str, hash: &str) -> schema::AuthorizeCompositionInput {
        schema::AuthorizeCompositionInput {
            operation: operation.to_owned(),
            inputSha256: hash.to_owned(),
        }
    }

    fn hash() -> String {
        "0".repeat(64)
    }

    #[test]
    fn a_person_with_the_permission_is_named_with_what_they_asked_for() {
        let answer = authorize(&person("admin", &["sms:send"]), args("send", &hash())).unwrap();
        assert_eq!(answer.userId, "u1");
        assert_eq!(answer.role, "admin");
        assert_eq!(answer.operation, "send");
        assert_eq!(answer.inputSha256, hash());
    }

    #[test]
    fn a_person_without_the_permission_is_forbidden() {
        let error = authorize(&person("admin", &["sms:read"]), args("send", &hash())).unwrap_err();
        assert!(matches!(error, CratestackError::Forbidden(_)), "{error:?}");
    }

    #[test]
    fn an_app_is_forbidden_whatever_scope_it_holds() {
        let mut ctx = Principal {
            sub: "appc_x".to_owned(),
            kind: PrincipalKind::App,
            role: "app".to_owned(),
            app_id: "app1".to_owned(),
        }
        .into_context();
        ctx.extensions
            .insert("scope".to_owned(), Value::String(" sms:send ".to_owned()));
        let error = authorize(&ctx, args("send", &hash())).unwrap_err();
        assert!(matches!(error, CratestackError::Forbidden(_)), "{error:?}");
    }

    #[test]
    fn an_unknown_operation_or_a_malformed_hash_is_a_validation_error() {
        let ctx = person("admin", &["sms:send"]);
        for bad in [
            args("delete", &hash()),
            args("send", "abc"),
            args("", &hash()),
        ] {
            let error = authorize(&ctx, bad).unwrap_err();
            assert!(matches!(error, CratestackError::Validation(_)), "{error:?}");
        }
    }
}
