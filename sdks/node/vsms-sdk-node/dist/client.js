import { SdkError } from "./error.js";
import { PrivateKeyJwtTokenStore, } from "./token.js";
const IDEMPOTENCY_REPLAYED_HEADER = "idempotency-replayed";
const IDEMPOTENCY_KEY_HEADER = "idempotency-key";
export class VsmsClient {
    #baseUrl;
    #tokenStore;
    constructor(options) {
        this.#baseUrl = options.baseUrl.replace(/\/+$/, "");
        this.#tokenStore = options.tokenStore;
    }
    static privateKeyJwt(options) {
        const tokenStore = new PrivateKeyJwtTokenStore({
            issuer: options.issuer,
            clientId: options.clientId,
            scope: options.scope,
            audience: options.audience,
        }, {
            keyPath: options.keyPath,
            pemString: options.privateKeyPem,
        });
        return new VsmsClient({ baseUrl: options.issuer, tokenStore });
    }
    get tokenStore() {
        return this.#tokenStore;
    }
    /**
     * Helper that calls an authenticated endpoint, with automatic bounded 401 retry.
     */
    async #fetchWithAuth(path, init) {
        const url = `${this.#baseUrl}${path.startsWith("/") ? path : `/${path}`}`;
        const makeRequest = async () => {
            const token = await this.#tokenStore.get();
            const headers = new Headers(init.headers);
            headers.set("authorization", `Bearer ${token}`);
            if (!headers.has("accept")) {
                headers.set("accept", "application/json");
            }
            return await fetch(url, {
                ...init,
                headers,
            });
        };
        let res = await makeRequest();
        if (res.status === 401) {
            await this.#tokenStore.invalidate();
            res = await makeRequest();
        }
        return res;
    }
    /**
     * `POST /$procs/sendMessage`. Handles token acquisition, caching, bounded 401 retry,
     * and optional `Idempotency-Key` header with replay tracking.
     */
    async sendMessage(input, options) {
        const headers = {
            "content-type": "application/json",
            accept: "application/json",
        };
        if (options?.idempotencyKey) {
            headers[IDEMPOTENCY_KEY_HEADER] = options.idempotencyKey;
        }
        const response = await this.#fetchWithAuth("/$procs/sendMessage", {
            method: "POST",
            headers,
            body: JSON.stringify({ args: input }),
        });
        const text = await response.text();
        const isReplayed = response.headers.get(IDEMPOTENCY_REPLAYED_HEADER) === "true";
        if (!response.ok) {
            let parsed;
            try {
                parsed = JSON.parse(text);
            }
            catch {
                // Fallback for non-JSON errors (e.g. text/plain idempotency in flight message)
            }
            throw new SdkError(parsed?.message ?? text ?? `sendMessage failed with status ${response.status}`, {
                httpStatus: response.status,
                gatewayCode: parsed?.code,
                fieldErrors: typeof parsed?.details === "object" && parsed.details !== null
                    ? parsed.details
                    : undefined,
            });
        }
        let result;
        try {
            result = JSON.parse(text);
        }
        catch (err) {
            throw new SdkError("failed to parse sendMessage response as JSON", {
                cause: err,
                httpStatus: response.status,
            });
        }
        return {
            result,
            idempotencyReplayed: isReplayed,
        };
    }
    /**
     * `POST /$procs/previewMessage`.
     */
    async previewMessage(input) {
        const response = await this.#fetchWithAuth("/$procs/previewMessage", {
            method: "POST",
            headers: {
                "content-type": "application/json",
                accept: "application/json",
            },
            body: JSON.stringify({ args: input }),
        });
        const text = await response.text();
        if (!response.ok) {
            let parsed;
            try {
                parsed = JSON.parse(text);
            }
            catch {
                // fallback
            }
            throw new SdkError(parsed?.message ?? text ?? `previewMessage failed with status ${response.status}`, {
                httpStatus: response.status,
                gatewayCode: parsed?.code,
            });
        }
        return JSON.parse(text);
    }
    /**
     * `GET /messages/{id}`.
     */
    async getMessage(id) {
        const response = await this.#fetchWithAuth(`/messages/${encodeURIComponent(id)}`, {
            method: "GET",
            headers: {
                accept: "application/json",
            },
        });
        const text = await response.text();
        if (!response.ok) {
            let parsed;
            try {
                parsed = JSON.parse(text);
            }
            catch {
                // fallback
            }
            throw new SdkError(parsed?.message ?? text ?? `GET /messages/${id} failed with status ${response.status}`, {
                httpStatus: response.status,
                gatewayCode: parsed?.code,
            });
        }
        return JSON.parse(text);
    }
}
//# sourceMappingURL=client.js.map