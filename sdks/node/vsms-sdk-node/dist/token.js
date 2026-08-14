import { randomUUID } from "node:crypto";
import { readFileSync } from "node:fs";
import { importPKCS8, SignJWT } from "jose";
import { SdkError } from "./error.js";
const ASSERTION_TTL_SECONDS = 60;
const CLIENT_ASSERTION_TYPE = "urn:ietf:params:oauth:client-assertion-type:jwt-bearer";
const DEFAULT_TOKEN_TTL_SECONDS = 15 * 60;
const EXPIRY_SAFETY_MARGIN_SECONDS = 60;
export class PrivateKeyJwtTokenStore {
    #issuer;
    #tokenEndpoint;
    #clientId;
    #scope;
    #audience;
    #signingKeyPromise;
    #cached = null;
    #inFlight = null;
    constructor(options, keyMaterial) {
        this.#issuer = options.issuer.replace(/\/+$/, "");
        this.#tokenEndpoint = `${this.#issuer}/token`;
        this.#clientId = options.clientId;
        this.#scope = options.scope ?? "sms:send sms:read";
        this.#audience = options.audience ?? "token_endpoint";
        this.#signingKeyPromise = (async () => {
            let pem = keyMaterial.pemString;
            if (pem === undefined && keyMaterial.keyPath !== undefined) {
                try {
                    pem = readFileSync(keyMaterial.keyPath, "utf8");
                }
                catch (err) {
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
            }
            catch (err) {
                throw new SdkError("invalid RSA private key PEM (PKCS#8 expected)", { cause: err });
            }
        })();
    }
    static fromKeyPath(issuer, clientId, keyPath, scope, audience) {
        return new PrivateKeyJwtTokenStore({ issuer, clientId, scope, audience }, { keyPath });
    }
    static fromKeyPem(issuer, clientId, pemString, scope, audience) {
        return new PrivateKeyJwtTokenStore({ issuer, clientId, scope, audience }, { pemString });
    }
    async #mintAssertion() {
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
    async #requestToken() {
        let assertion;
        try {
            assertion = await this.#mintAssertion();
        }
        catch (err) {
            if (err instanceof SdkError)
                throw err;
            throw new SdkError("failed to sign client assertion", { cause: err });
        }
        const body = new URLSearchParams({
            grant_type: "client_credentials",
            client_id: this.#clientId,
            client_assertion_type: CLIENT_ASSERTION_TYPE,
            client_assertion: assertion,
            scope: this.#scope,
        });
        let response;
        try {
            response = await fetch(this.#tokenEndpoint, {
                method: "POST",
                headers: { "content-type": "application/x-www-form-urlencoded" },
                body: body.toString(),
            });
        }
        catch (err) {
            throw new SdkError(`token request to ${this.#tokenEndpoint} failed: network error`, {
                cause: err,
            });
        }
        const text = await response.text();
        if (!response.ok) {
            throw new SdkError(`token request to ${this.#tokenEndpoint} failed (${response.status}): ${text}`, { httpStatus: response.status });
        }
        let parsed;
        try {
            parsed = JSON.parse(text);
        }
        catch (err) {
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
    async get() {
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
            }
            finally {
                this.#inFlight = null;
            }
        })();
        return await this.#inFlight;
    }
    invalidate() {
        this.#cached = null;
    }
}
//# sourceMappingURL=token.js.map