#!/usr/bin/env node
// vsms integration example (Node.js): the full HTTP path a third-party
// backend uses to send one message through vsms — no admin-console code
// imported, no @vsms/sms-client, nothing this file could not also do
// copied into a different repository entirely (see examples/README.md
// for why that's the deliberate choice here).
//
// 1. Read the PEM `sms-gateway provision-client` wrote.
// 2. Sign an RFC 7523 §3 `private_key_jwt` client assertion.
// 3. Exchange it at `POST {issuer}/token` for a `client_credentials`
//    access token.
// 4. Call `POST {issuer}/$procs/sendMessage` with that Bearer token.
// 5. Read the message back with `GET {issuer}/messages/{id}` and print
//    its state — proving the write actually landed, not just that the
//    mutation's own response claimed success.
//
// This mirrors packages/gateway/src/token.ts — the vsms admin console's
// own token acquisition — deliberately, rather than inventing a second
// interpretation of the same exchange. The three load-bearing details
// documented there apply here unchanged: `scope` is mandatory on the
// token request, `jti` is never reused, and the access token is cached
// until `exp - 60s`, not `exp`.
//
// Usage:
//   node src/index.mjs \
//     --issuer http://127.0.0.1:8080 \
//     --client-id <clientId that provision-client printed> \
//     --private-key-path /path/to/console-client-key.pem \
//     --to +237677123456 \
//     --sender-id VYMALO \
//     --body "Hello from the vsms Node example"
//
// Every flag also reads from an env var (VSMS_ISSUER, VSMS_CLIENT_ID,
// VSMS_PRIVATE_KEY_PATH, VSMS_SCOPE) so a real integration never has to
// hardcode a credential path in argv.

import { randomUUID } from "node:crypto";
import { readFileSync } from "node:fs";
import { parseArgs } from "node:util";
import { importPKCS8, SignJWT } from "jose";

const CLIENT_ASSERTION_TYPE = "urn:ietf:params:oauth:client-assertion-type:jwt-bearer";
// RFC 7523 client assertions are meant to be short-lived — long enough to
// reach /token, never long enough to be useful if intercepted in
// transit. Matches token.ts's own ASSERTION_TTL_SECONDS.
const ASSERTION_TTL_SECONDS = 60;
// Mint a fresh access token this many seconds before the cached one
// actually expires, so a request never starts with a token that dies
// mid-flight. Matches token.ts's own EXPIRY_SAFETY_MARGIN_SECONDS exactly.
const EXPIRY_SAFETY_MARGIN_SECONDS = 60;
// Used when the token response omits expires_in (optional in the OAuth2
// response shape). Matches token.ts's own fallback.
const DEFAULT_TOKEN_TTL_SECONDS = 15 * 60;

function parseCli() {
  const { values } = parseArgs({
    options: {
      issuer: { type: "string", default: process.env.VSMS_ISSUER ?? "http://127.0.0.1:8080" },
      "client-id": { type: "string", default: process.env.VSMS_CLIENT_ID },
      "private-key-path": { type: "string", default: process.env.VSMS_PRIVATE_KEY_PATH },
      scope: { type: "string", default: process.env.VSMS_SCOPE ?? "sms:send sms:read" },
      to: { type: "string" },
      "sender-id": { type: "string" },
      body: { type: "string", default: "Hello from the vsms Node.js integration example" },
      "client-ref": { type: "string" },
    },
  });

  const missing = ["client-id", "private-key-path", "to", "sender-id"].filter(
    (key) => !values[key],
  );
  if (missing.length > 0) {
    console.error(`missing required flag(s): ${missing.map((k) => `--${k}`).join(", ")}`);
    console.error(
      "usage: node src/index.mjs --client-id <id> --private-key-path <pem> --to <e164> " +
        "--sender-id <senderId> [--body <text>] [--client-ref <key>] [--issuer <url>] " +
        "[--scope <scopes>]",
    );
    process.exit(1);
  }

  return {
    issuer: values.issuer.replace(/\/+$/, ""),
    clientId: values["client-id"],
    privateKeyPath: values["private-key-path"],
    scope: values.scope,
    to: values.to,
    senderId: values["sender-id"],
    body: values.body,
    clientRef: values["client-ref"],
  };
}

/**
 * Mints and caches an access token, re-minting only once the cached one
 * is within EXPIRY_SAFETY_MARGIN_SECONDS of expiry. A single run of this
 * example only ever makes two authenticated calls (the send, then the
 * read-back), so caching barely matters here in isolation — but this is
 * the shape a real integration wants for the hundredth call, not just
 * the second, and it is a direct port of token.ts's own
 * getAccessToken/requestToken pair.
 */
class TokenCache {
  #tokenEndpoint;
  #clientId;
  #signingKeyPromise;
  #scope;
  #cached;

  constructor(issuer, clientId, privateKeyPem, scope) {
    this.#tokenEndpoint = `${issuer}/token`;
    this.#clientId = clientId;
    this.#signingKeyPromise = importPKCS8(privateKeyPem, "RS256");
    this.#scope = scope;
    this.#cached = null;
  }

  /**
   * A fresh RFC 7523 §3 client assertion, signed with the caller's own
   * private key.
   *
   * `kid` is the client id, matching token.ts exactly — `authkestra_op`'s
   * own `select_key` treats a single-key JWKS (which is all
   * `provisionAppClient` ever produces — see the main repo's AGENTS.md)
   * as unambiguous even without a `kid`, but setting it costs nothing.
   *
   * `aud` is the token endpoint URL, matching token.ts and per authkestra
   * 0.3.2+, which also accepts the bare issuer.
   *
   * `jti` is a fresh UUID on every call, never reused: `ClientAssertion`
   * is an insert-only table that replay-protects on this value at the
   * database (a 23505 unique-constraint violation on `record_jti`), so
   * resending the same assertion on a retry would collide with the
   * original attempt rather than repeating it.
   */
  async #mintAssertion() {
    const key = await this.#signingKeyPromise;
    const now = Math.floor(Date.now() / 1000);
    return new SignJWT({})
      .setProtectedHeader({ alg: "RS256", kid: this.#clientId })
      .setIssuer(this.#clientId)
      .setSubject(this.#clientId)
      .setAudience(this.#tokenEndpoint)
      .setJti(randomUUID())
      .setIssuedAt(now)
      .setExpirationTime(now + ASSERTION_TTL_SECONDS)
      .sign(key);
  }

  async #requestToken() {
    const assertion = await this.#mintAssertion();
    const body = new URLSearchParams({
      grant_type: "client_credentials",
      client_id: this.#clientId,
      client_assertion_type: CLIENT_ASSERTION_TYPE,
      client_assertion: assertion,
      // Mandatory, not optional: omitting `scope` does not fall back to
      // the client's registered scopes, it mints a token with
      // `scope: null`, and this deployment's Layer-2 RBAC treats a
      // missing scope as denial. Same footgun token.ts's own module doc
      // calls out.
      scope: this.#scope,
    });

    const response = await fetch(this.#tokenEndpoint, {
      method: "POST",
      headers: { "content-type": "application/x-www-form-urlencoded" },
      body: body.toString(),
    });

    const text = await response.text();
    if (!response.ok) {
      throw new Error(
        `token request to ${this.#tokenEndpoint} failed (${response.status}): ${text}`,
      );
    }

    const parsed = JSON.parse(text);
    console.log(
      `minted access token (scope=${parsed.scope ?? "null"}, expires in ${parsed.expires_in ?? DEFAULT_TOKEN_TTL_SECONDS}s)`,
    );
    const ttlSeconds = parsed.expires_in ?? DEFAULT_TOKEN_TTL_SECONDS;
    return {
      accessToken: parsed.access_token,
      expiresAtMs: Date.now() + Math.max(ttlSeconds - EXPIRY_SAFETY_MARGIN_SECONDS, 0) * 1000,
    };
  }

  async get() {
    if (this.#cached != null && this.#cached.expiresAtMs > Date.now()) {
      return this.#cached.accessToken;
    }
    this.#cached = await this.#requestToken();
    return this.#cached.accessToken;
  }
}

async function sendMessage(issuer, accessToken, { to, body, senderId, clientRef }) {
  const args = { to, body, senderId };
  if (clientRef) {
    args.clientRef = clientRef;
  }

  const response = await fetch(`${issuer}/$procs/sendMessage`, {
    method: "POST",
    headers: {
      authorization: `Bearer ${accessToken}`,
      "content-type": "application/json",
      accept: "application/json",
    },
    body: JSON.stringify({ args }),
  });

  const text = await response.text();
  if (response.status === 409) {
    console.log(
      "sendMessage returned 409 Conflict — if --client-ref was passed, that clientRef was " +
        "already used on a prior send. This is clientRef's database-level dedupe doing " +
        `exactly its job, not a bug to retry around: ${text}`,
    );
    return null;
  }
  if (!response.ok) {
    throw new Error(`sendMessage failed (${response.status}): ${text}`);
  }
  return JSON.parse(text);
}

async function getMessage(issuer, accessToken, messageId) {
  const response = await fetch(`${issuer}/messages/${messageId}`, {
    headers: { authorization: `Bearer ${accessToken}`, accept: "application/json" },
  });
  const text = await response.text();
  if (!response.ok) {
    throw new Error(`GET /messages/${messageId} failed (${response.status}): ${text}`);
  }
  return JSON.parse(text);
}

async function main() {
  const cli = parseCli();
  const privateKeyPem = readFileSync(cli.privateKeyPath, "utf8");
  const tokens = new TokenCache(cli.issuer, cli.clientId, privateKeyPem, cli.scope);

  const accessToken = await tokens.get();

  const sent = await sendMessage(cli.issuer, accessToken, cli);
  if (sent == null) {
    return;
  }
  console.log();
  console.log(
    `sent: messageId=${sent.messageId} state=${sent.state} encoding=${sent.encoding} ` +
      `segments=${sent.segments} operator=${sent.operator} estimatedCostXaf=${sent.estimatedCostXaf}`,
  );

  // Prove the write actually landed — read it back through the REST
  // surface rather than trusting the mutation's own echoed response.
  const readBackToken = await tokens.get();
  const message = await getMessage(cli.issuer, readBackToken, sent.messageId);
  console.log(
    `read back: id=${message.id} state=${message.state} providerMessageRef=${message.providerMessageRef}`,
  );
}

main().catch((error) => {
  console.error(error.message ?? error);
  process.exitCode = 1;
});
