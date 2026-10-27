//! The signed transport's server keys (VS-3): the flags `serve` and
//! `server-keys` share, and the loader that refuses to boot without them.
//!
//! The server holds two Ed25519 keys, both PKCS#8 PEM files
//! (`openssl genpkey -algorithm ed25519`): `--envelope-signing-key-file`
//! signs every response, and `--envelope-next-signing-key-file` is the key the
//! next rotation promotes. Clients pin the public half of **both**, so a
//! rotation needs no flag day. See `sms_api::envelope`'s module doc.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use sms_api::envelope::ServerKeys;
use tracing::info;

/// The two key files. `#[command(flatten)]`ed into `serve` and `server-keys`,
/// so the flag names, env vars and help text exist once. Both are required:
/// there is no default key and no unsigned mode, so a deployment that forgot
/// them fails at the argument parser, before it opens a socket.
///
/// The values are paths, not secrets, so unlike a password flag they need no
/// `hide_env_values`; the files are the secret and must be mounted `0400`.
#[derive(Debug, clap::Args)]
pub(crate) struct EnvelopeKeyArgs {
    /// PKCS#8 PEM Ed25519 private key that signs every response. Clients
    /// verify answers against its public key (`sms-gateway server-keys`).
    #[arg(long, env = "SMS_ENVELOPE_SIGNING_KEY_FILE")]
    pub(crate) envelope_signing_key_file: PathBuf,

    /// PKCS#8 PEM Ed25519 private key for the next rotation. Never signs
    /// anything yet; its public key is what clients pin beside the current
    /// one so promoting it later breaks nobody. Must differ from
    /// `--envelope-signing-key-file`.
    #[arg(long, env = "SMS_ENVELOPE_NEXT_SIGNING_KEY_FILE")]
    pub(crate) envelope_next_signing_key_file: PathBuf,
}

impl EnvelopeKeyArgs {
    /// Reads and checks both files.
    ///
    /// # Errors
    ///
    /// A file that cannot be read, is not a PKCS#8 Ed25519 key, or the same
    /// key twice. The message names the flag and the path, never the content.
    pub(crate) fn load(&self) -> Result<ServerKeys> {
        load(
            &self.envelope_signing_key_file,
            &self.envelope_next_signing_key_file,
        )
    }
}

fn load(current: &Path, next: &Path) -> Result<ServerKeys> {
    let read = |path: &Path, flag: &str| {
        std::fs::read(path).with_context(|| format!("reading {flag} {}", path.display()))
    };
    let current_pem = read(current, "--envelope-signing-key-file")?;
    let next_pem = read(next, "--envelope-next-signing-key-file")?;
    ServerKeys::from_pems(&current_pem, &next_pem).with_context(|| {
        format!(
            "the envelope signing keys ({} and {})",
            current.display(),
            next.display()
        )
    })
}

/// `sms-gateway server-keys`: prints the two public keys an integrator pins.
/// Needs no database.
#[derive(Debug, clap::Args)]
pub(crate) struct ServerKeysArgs {
    #[command(flatten)]
    pub(crate) keys: EnvelopeKeyArgs,
}

/// Prints `current: <base64url>` and `next: <base64url>`, the raw 32-byte
/// Ed25519 public keys, one per line, and nothing else on stdout.
pub(crate) fn server_keys_command(args: &ServerKeysArgs) -> Result<()> {
    let keys = args.keys.load()?;
    print!("{}", keys.report());
    Ok(())
}

/// What `serve` logs at boot: the public keys, which are not secret and are
/// the first thing to compare when a client reports an unverifiable answer.
pub(crate) fn log_public_keys(keys: &ServerKeys) {
    let report = keys.report();
    let mut lines = report.lines();
    info!(
        current = lines.next().unwrap_or_default(),
        next = lines.next().unwrap_or_default(),
        "envelope signing keys loaded (public halves)"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use sms_api::envelope::pem_of_seed;

    fn write(dir: &Path, name: &str, seed: u8) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, pem_of_seed(&[seed; 32])).unwrap();
        path
    }

    fn scratch(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("sms-server-keys-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn two_distinct_keys_load_and_report_both_public_halves() {
        let dir = scratch("ok");
        let keys = load(&write(&dir, "a.pem", 1), &write(&dir, "b.pem", 2)).unwrap();
        let (current, next) = keys.public_keys();
        assert_ne!(current, next);
        assert_eq!(keys.report().lines().count(), 2);
    }

    #[test]
    fn the_same_key_in_both_files_refuses_to_load() {
        let dir = scratch("same");
        let error = load(&write(&dir, "a.pem", 3), &write(&dir, "b.pem", 3))
            .err()
            .unwrap();
        assert!(format!("{error:#}").contains("same key"), "{error:#}");
    }

    #[test]
    fn a_missing_file_names_the_flag_and_the_path() {
        let dir = scratch("missing");
        let error = load(&dir.join("absent.pem"), &write(&dir, "b.pem", 2))
            .err()
            .unwrap();
        let text = format!("{error:#}");
        assert!(text.contains("--envelope-signing-key-file"), "{text}");
        assert!(text.contains("absent.pem"), "{text}");
    }

    /// `serve` with the flags it needs apart from the two key files, plus
    /// `extra`.
    fn parse_serve(extra: &[&str]) -> Result<crate::Cli, clap::Error> {
        use clap::Parser;

        let mut args = vec![
            "sms-gateway",
            "serve",
            "--database-url",
            "postgres://x",
            "--hash-pepper",
            "p",
        ];
        args.extend_from_slice(extra);
        crate::Cli::try_parse_from(args)
    }

    /// `serve` is `Required`-or-nothing: no flag, no boot, for each of the two
    /// files on its own - a default on either would let a deployment boot
    /// without having chosen its keys.
    #[test]
    fn serve_does_not_parse_without_the_key_flags() {
        const CURRENT: &str = "--envelope-signing-key-file";
        const NEXT: &str = "--envelope-next-signing-key-file";

        // The arguments clap reports as missing - not the usage line, which
        // names both flags whatever was passed.
        let missing = |extra: &[&str]| -> Vec<String> {
            use clap::error::{ContextKind, ContextValue};

            let error = parse_serve(extra).expect_err("serve needs both key files");
            assert_eq!(
                error.kind(),
                clap::error::ErrorKind::MissingRequiredArgument
            );
            match error.get(ContextKind::InvalidArg) {
                Some(ContextValue::Strings(names)) => names.clone(),
                other => panic!("clap did not name the missing arguments: {other:?}"),
            }
        };
        let names = |missing: &[String], flag: &str| {
            missing
                .iter()
                .any(|name| name.starts_with(&format!("{flag} ")))
        };

        let neither = missing(&[]);
        assert!(
            names(&neither, CURRENT) && names(&neither, NEXT),
            "{neither:?}"
        );

        let only_current = missing(&[CURRENT, "current.pem"]);
        assert!(names(&only_current, NEXT), "{only_current:?}");
        assert!(!names(&only_current, CURRENT), "{only_current:?}");

        let only_next = missing(&[NEXT, "next.pem"]);
        assert!(names(&only_next, CURRENT), "{only_next:?}");
        assert!(!names(&only_next, NEXT), "{only_next:?}");

        // The control: with both, it parses.
        assert!(parse_serve(&[CURRENT, "current.pem", NEXT, "next.pem"]).is_ok());
    }

    /// VS-4: the envelope keys are the only keys `serve` takes. The embedded
    /// `OpenID` Provider's `--issuer` (and its signing key, which had no flag
    /// but made `serve` refuse to boot) are gone, and a stale deployment that
    /// still passes `--issuer` is told so by the parser rather than ignored.
    #[test]
    fn serve_takes_no_issuer() {
        let error = parse_serve(&[
            "--envelope-signing-key-file",
            "current.pem",
            "--envelope-next-signing-key-file",
            "next.pem",
            "--issuer",
            "http://localhost",
        ])
        .expect_err("--issuer no longer exists");
        assert_eq!(error.kind(), clap::error::ErrorKind::UnknownArgument);
    }
}
