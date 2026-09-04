//! `Command::Healthcheck` — see that variant's own doc comment in `main.rs`
//! for why this exists at all and why it's a hand-rolled HTTP/1.1 GET.

use anyhow::{Context, Result, bail};

/// `Command::Healthcheck`'s flags. See `Command::Healthcheck`'s own doc
/// comment in `main.rs` — the enum variant carries the "why", this struct
/// only carries the flags themselves.
#[derive(Debug, clap::Args)]
pub(crate) struct HealthcheckArgs {
    #[arg(long, default_value = "127.0.0.1:8080")]
    pub(crate) addr: String,

    #[arg(long, default_value = "/healthz")]
    pub(crate) path: String,
}

/// `Command::Healthcheck`'s body — see that variant's own doc comment for
/// why this exists at all (a distroless `static` image has no shell or
/// `curl` for the container/orchestrator health check to shell out to) and
/// why it's a hand-rolled HTTP/1.1 GET rather than pulling in `reqwest`:
/// this only ever needs to run against `127.0.0.1`, in-process, so a raw
/// socket is simpler than standing up a TLS-capable client for a plaintext
/// loopback request.
pub(crate) fn healthcheck_command(addr: &str, path: &str) -> Result<()> {
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::time::Duration;

    let mut stream = TcpStream::connect(addr)
        .with_context(|| format!("connecting to {addr} for healthcheck"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .context("setting healthcheck read timeout")?;
    stream
        .set_write_timeout(Some(Duration::from_secs(3)))
        .context("setting healthcheck write timeout")?;
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
    )
    .context("writing healthcheck request")?;

    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .context("reading healthcheck response")?;
    let status_line = response
        .lines()
        .next()
        .context("empty healthcheck response")?;
    // e.g. "HTTP/1.1 200 OK" — the status code is always the second
    // whitespace-delimited token of the status line (RFC 9112 §4).
    if status_line.split_whitespace().nth(1) == Some("200") {
        Ok(())
    } else {
        bail!("unhealthy: GET {addr}{path} returned {status_line:?}")
    }
}
