//! `Command::PairConsoleKey` - see that variant's own doc comment in
//! `main.rs` for what this does and why it exists (signed transport, VS-5a).

use anyhow::{Context, Result};
use chrono::Utc;
use cratestack::sqlx::postgres::PgPoolOptions;
use sms_api::console_keys::pair_existing_user;
use sms_api::schema::Cratestack;
use sms_api::system_context;

/// `Command::PairConsoleKey`'s flags. See `Command::PairConsoleKey`'s own doc
/// comment in `main.rs` - the enum variant carries the "why", this struct only
/// carries the flags themselves.
#[derive(Debug, clap::Args)]
pub(crate) struct PairConsoleKeyArgs {
    #[arg(long, env = "DATABASE_URL", hide_env_values = true)]
    pub(crate) database_url: String,

    /// The person the key is for: their email, or their `User.id`. They must
    /// already exist and be active - this command never creates a user, so it
    /// cannot create an owner.
    #[arg(long)]
    pub(crate) user: String,

    /// The console key's public half: the SEC1 uncompressed P-256 point (65
    /// bytes, `0x04 || X || Y`), base64url without padding - what a browser's
    /// `crypto.subtle.exportKey("raw", publicKey)` returns. Not secret, so it
    /// is a plain flag. base64url's alphabet includes `-`, so a value can
    /// start with one and clap would read it as an unknown flag, exactly the
    /// `register-client` defect: `allow_hyphen_values` keeps it a value. (An
    /// uncompressed P-256 point starts `0x04`, i.e. `B`, so a real key never
    /// does; the attribute is for the day the encoding changes, and the unit
    /// test below parses such a value on purpose.)
    #[arg(long, allow_hyphen_values = true)]
    pub(crate) public_key: String,

    /// A human-readable label for the key (1-64 characters), e.g.
    /// `"break-glass, 2026-10"`.
    #[arg(long)]
    pub(crate) label: Option<String>,
}

/// `Command::PairConsoleKey`'s body. The key is identified by its RFC 9679
/// thumbprint, so the same key registered twice leaves one row and exits 0.
pub(crate) async fn pair_console_key_command(args: PairConsoleKeyArgs) -> Result<()> {
    let PairConsoleKeyArgs {
        database_url,
        user,
        public_key,
        label,
    } = args;

    // Before the database, so a typo'd key fails with nothing touched.
    sms_api::console_keys::parse_public_key(&public_key).context("--public-key")?;
    if user.trim().is_empty() {
        anyhow::bail!("--user must not be empty - pass the person's email or User.id");
    }

    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect(&database_url)
        .await
        .context("connecting to Postgres")?;
    let db = Cratestack::builder(pool).build();
    let sys = system_context("sms-gateway:pair-console-key");

    let (key, found, created) =
        pair_existing_user(&db, &sys, &user, &public_key, label, Utc::now())
            .await
            .context("registering the console key")?;

    println!(
        "user: {} (id={}, role={})",
        found.email, found.id, found.roleKey
    );
    println!("key id: {}", key.id);
    println!("thumbprint: {}", key.thumbprint);
    println!("expires at: {}", key.expiresAt.to_rfc3339());
    println!(
        "{}",
        if created {
            "registered"
        } else {
            "already registered - nothing to do"
        }
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    #[derive(clap::Parser)]
    struct Cli {
        #[command(flatten)]
        args: super::PairConsoleKeyArgs,
    }

    #[test]
    fn a_public_key_that_starts_with_a_hyphen_is_a_value_not_a_flag() {
        use clap::Parser as _;
        let key = "-S0mEbase64urlKeyThatStartsWithAHyphen_xxxxxx";
        let cli = Cli::try_parse_from([
            "pair-console-key",
            "--database-url",
            "postgres://unused",
            "--user",
            "ada@example.test",
            "--public-key",
            key,
        ])
        .expect("a leading '-' is part of a base64url value");
        assert_eq!(cli.args.public_key, key);
    }
}
