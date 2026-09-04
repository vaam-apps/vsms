//! `Command::Routes`'s body — moved verbatim out of `main`'s own `match`
//! (cleanup PR D). See `Command::Routes`'s own doc comment in `main.rs`
//! for why this needs no database.

use anyhow::Result;

// `Result<()>` here is genuinely necessary, not just habit: every other
// `Command` arm in `main`'s own dispatch `match` is `async fn(...) ->
// Result<()>`, and this arm has to return the identical type for the
// `match` to type-check — this function can never itself fail, but its
// caller's uniform return type requires the wrap.
#[allow(clippy::unnecessary_wraps)]
pub(crate) fn run() -> Result<()> {
    let routes = sms_api::route_table();
    println!("{} generated routes:", routes.len());
    for (method, path) in routes {
        println!("  {method:<7} {path}");
    }
    Ok(())
}
