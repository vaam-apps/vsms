//! Helpers shared by more than one `commands::*` submodule — see
//! `commands.md` for why everything else lives one file per subcommand
//! instead of here.

use sms_api::{Principal, PrincipalKind};

/// The `system`-role context every OP-adjacent database write in this
/// binary runs under — never handed to a caller, matching
/// `Procedures::sys()`'s own convention.
pub(crate) fn system_context() -> cratestack::CratestackContext {
    Principal {
        sub: "sms-gateway:op".to_owned(),
        kind: PrincipalKind::App,
        role: "system".to_owned(),
        app_id: String::new(),
    }
    .into_context()
}
