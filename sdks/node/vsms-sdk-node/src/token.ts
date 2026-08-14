import { randomUUID } from "node:crypto";
import { readFileSync } from "node:fs";
import { importPKCS8, SignJWT } from "jose";
import { SdkError } from "./error.js";

const ASSERTION_TTL_SECONDS = 60;
const CLIENT_ASSERTION_TYPE = "urn:ietf:params:oauth:client-assertion-type:jwt-bearer";
const DEFAULT_TOKEN_TTL_SECONDS = 15 * 60;
const EXPIRY_SAFETY_MARGIN_SECONDS = 60;

export type TokenAudience = "token_endpoint" | "issuer";

export interface PrivateKeyJwtConfigOptions {
  issuer: string;
  clientId: string;
  scope?: string | undefined;
  audience?: TokenAudience | undefined;
}

export interface CachedToken {
  accessToken: string;
  expiresAtMs: number;
}

interface TokenResponse {
  access_token: string;
  token_type?: string;
  expires_in?: number;
  scope?: string;
}

export interface TokenStore {
  get(): Promise<string>;
  invalidate(): Promise<void> | void;
}

type KeyLike = Awaited<ReturnType<typeof importPKCS8>>;

export class PrivateKeyJwtTokenStore implements TokenStore {
  readonly #issuer: string;
  readonly #tokenEndpoint: string;
  readonly #clientId: string;
  readonly #scope: string;
  readonly #audience: TokenAudience;
  readonly #signingKeyPromise: Promise<KeyLike>;
  #cached: CachedToken | null = null;
  #inFlight: Promise<string> | null = null;

  constructor(
    options: PrivateKeyJwtConfigOptions,
    keyMaterial: { pemString?: string | undefined; keyPath?: string | undefined },
  ) {
    this.#issuer = options.issuer.replace(/\/+$/, "");
    this.#tokenEndpoint = `${this.#issuer}/token`;
    this.#clientId = options.clientId;
    const rawScope = options.scope ?? "sms:send sms:read";
    const trimmedScope = rawScope.trim();
    if (trimmedScope.length === 0) {
      throw new SdkError(
        "scope cannot be empty (must request explicit permissions, e.g. 'sms:send sms:read')",
      );
    }
    this.#scope = trimmedScope;
    this.#audience = options.audience ?? "token_endpoint";

    this.#signingKeyPromise = (async () => {
      let pem = keyMaterial.pemString;
      if (pem === undefined && keyMaterial.keyPath !== undefined) {
        try {
          pem = readFileSync(keyMaterial.keyPath, "utf8");
        } catch (err) {
          throw new SdkError(`failed to read private key at ${keyMaterial.keyPath}`, {
            cause: err,
          });
        }
      }
      if (!pem) {
        throw new SdkError("no private key PEM or key path provided");
      }
      try {
        return await importPKCS8(pem, "RS256");
      } catch (err) {
        throw new SdkError("invalid RSA private key PEM (PKCS#8 expected)", { cause: err });
      }
    })();
  }

  static fromKeyPath(
    issuer: string,
    clientId: string,
    keyPath: string,
    scope?: string | undefined,
    audience?: TokenAudience | undefined,
  ): PrivateKeyJwtTokenStore {
    return new PrivateKeyJwtTokenStore({ issuer, clientId, scope, audience }, { keyPath });
  }

  static fromKeyPem(
    issuer: string,
    clientId: string,
    pemString: string,
    scope?: string | undefined,
    audience?: TokenAudience | undefined,
  ): PrivateKeyJwtTokenStore {
    return new PrivateKeyJwtTokenStore({ issuer, clientId, scope, audience }, { pemString });
  }

  async #mintAssertion(): Promise<string> {
    const key = await this.#signingKeyPromise;
    const now = Math.floor(Date.now() / 1000);
    const aud = this.#audience === "issuer" ? this.#issuer : this.#tokenEndpoint;

    return await new SignJWT({})
      .setProtectedHeader({ alg: "RS256", kid: this.#clientId })
      .setIssuer(this.#clientId)
      .setSubject(this.#clientId)
      .setAudience(aud)
      .setJti(randomUUID())
      .setIssuedAt(now)
      .setExpirationTime(now + ASSERTION_TTL_SECONDS)
      .sign(key);
  }

  async #requestToken(): Promise<CachedToken> {
    let assertion: string;
    try {
      assertion = await this.#mintAssertion();
    } catch (err) {
      if (err instanceof SdkError) throw err;
      throw new SdkError("failed to sign client assertion", { cause: err });
    }

    const body = new URLSearchParams({
      grant_type: "client_credentials",
      client_id: this.#clientId,
      client_assertion_type: CLIENT_ASSERTION_TYPE,
      client_assertion: assertion,
      scope: this.#scope,
    });

    let response: Response;
    try {
      response = await fetch(this.#tokenEndpoint, {
        method: "POST",
        headers: { "content-type": "application/x-www-form-urlencoded" },
        body: body.toString(),
      });
    } catch (err) {
      throw new SdkError(`token request to ${this.#tokenEndpoint} failed: network error`, {
        cause: err,
      });
    }

    const text = await response.text();
    if (!response.ok) {
      throw new SdkError(
        `token request to ${this.#tokenEndpoint} failed (${response.status}): ${text}`,
        { httpStatus: response.status },
      );
    }

    let parsed: TokenResponse;
    try {
      parsed = JSON.parse(text) as TokenResponse;
    } catch (err) {
      throw new SdkError(`token response from ${this.#tokenEndpoint} is not valid JSON: ${text}`, {
        cause: err,
      });
    }

    const ttlSeconds = parsed.expires_in ?? DEFAULT_TOKEN_TTL_SECONDS;
    return {
      accessToken: parsed.access_token,
      expiresAtMs: Date.now() + Math.max(ttlSeconds - EXPIRY_SAFETY_MARGIN_SECONDS, 0) * 1000,
    };
  }

  async get(): Promise<string> {
    if (this.#cached != null && this.#cached.expiresAtMs > Date.now()) {
      return this.#cached.accessToken;
    }

    if (this.#inFlight) {
      return await this.#inFlight;
    }

    this.#inFlight = (async () => {
      try {
        const token = await this.#requestToken();
        this.#cached = token;
        return token.accessToken;
      } finally {
        this.#inFlight = null;
      }
    })();

    return await this.#inFlight;
  }

  invalidate(): void {
    this.#cached = null;
  }
}
