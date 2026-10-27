import type { Agent as HttpAgent } from "node:http";
import type { Agent as HttpsAgent } from "node:https";
import { SdkError } from "./error.js";
import { type IdentityOptions, resolveSealer } from "./identity.js";
import { type CallContext, type OpenedAnswer, performSealedCall } from "./sealed-call.js";
import type {
  Message,
  PreviewInput,
  PreviewResult,
  SendMessageInput,
  SendMessageResult,
} from "./types.js";

/** Default timeout for every call, in milliseconds. */
export const DEFAULT_TIMEOUT_MS = 30_000;

const IDEMPOTENCY_REPLAYED_HEADER = "idempotency-replayed";

export interface SendMessageOptions {
  idempotencyKey?: string | undefined;
}

export interface SendMessageOutcome {
  result: SendMessageResult;
  /**
   * Whether the gateway answered from a prior call under the same
   * `Idempotency-Key` instead of executing again. Read from a response header,
   * which the signature does **not** cover: informational. The authoritative
   * statement that a retry did not duplicate the message is that both answers
   * carry the same `messageId`.
   */
  idempotencyReplayed: boolean;
}

export interface VsmsClientOptions extends IdentityOptions {
  /** The gateway's externally reachable origin, e.g. `https://sms.example.com`. */
  baseUrl: string;
  /** Default {@link DEFAULT_TIMEOUT_MS}. */
  timeoutMs?: number | undefined;
  /** An `http.Agent`/`https.Agent` for a proxy or a pool. Defaults to the global agent. */
  agent?: HttpAgent | HttpsAgent | undefined;
}

interface CratestackErrorResponseBody {
  code?: string | undefined;
  message?: string | undefined;
  details?: unknown;
}

/**
 * A vsms client. Every request is signed with the app's Ed25519 key and every
 * answer is verified against the gateway's pinned keys, so there is nothing to
 * acquire, cache or refresh. Build one with the key and the pinned keys:
 *
 * @example
 * const client = new VsmsClient({
 *   baseUrl: "https://sms.example.com",
 *   keyPath: "/run/secrets/vsms-signing-key.pem",
 *   serverKeys: parseServerKeysReport(readFileSync("server-keys.txt", "utf8")),
 * });
 */
export class VsmsClient {
  readonly #context: CallContext;

  constructor(options: VsmsClientOptions) {
    if (typeof options.baseUrl !== "string" || options.baseUrl.length === 0) {
      throw new SdkError("baseUrl is required", { kind: "config" });
    }
    const timeoutMs = options.timeoutMs ?? DEFAULT_TIMEOUT_MS;
    if (!Number.isInteger(timeoutMs) || timeoutMs <= 0) {
      throw new SdkError(`timeoutMs must be a positive integer, got ${timeoutMs}`, {
        kind: "config",
      });
    }
    this.#context = {
      baseUrl: options.baseUrl.replace(/\/+$/, ""),
      timeoutMs,
      agent: options.agent,
      sealer: resolveSealer(options),
    };
  }

  /** The registered public half of this client's key, unpadded base64url: what `sms-gateway register-client --public-key` takes. */
  get publicKey(): string {
    return this.#context.sealer.publicKey;
  }

  async #call(name: string, spec: Parameters<typeof performSealedCall>[1]): Promise<OpenedAnswer> {
    const answer = await performSealedCall(this.#context, spec);
    if (answer.status >= 200 && answer.status < 300) {
      return answer;
    }
    let parsed: CratestackErrorResponseBody | undefined;
    try {
      parsed = JSON.parse(answer.text) as CratestackErrorResponseBody;
    } catch {
      // Not the standard error JSON (e.g. the idempotency layer's text/plain
      // "in flight" message): what the gateway said is the message.
    }
    throw new SdkError(
      parsed?.message ?? (answer.text || `${name} failed with status ${answer.status}`),
      {
        kind: "remote",
        httpStatus: answer.status,
        gatewayCode: parsed?.code,
        fieldErrors:
          typeof parsed?.details === "object" && parsed.details !== null
            ? (parsed.details as Record<string, string[]>)
            : undefined,
      },
    );
  }

  #decode<T>(text: string, status: number, name: string): T {
    try {
      return JSON.parse(text) as T;
    } catch (err) {
      throw new SdkError(`failed to parse ${name} response as JSON: ${text}`, {
        kind: "decode",
        cause: err,
        httpStatus: status,
      });
    }
  }

  /**
   * `POST /$procs/sendMessage`, sealed. `options.idempotencyKey`, when given, is
   * sent as the `Idempotency-Key` header, which the signature binds: the
   * gateway then guarantees a repeated call under the same key within its TTL
   * never executes `sendMessage` twice. Each attempt is a freshly sealed
   * request (a new nonce), and this method never retries one itself.
   */
  async sendMessage(
    input: SendMessageInput,
    options?: SendMessageOptions | undefined,
  ): Promise<SendMessageOutcome> {
    const answer = await this.#call("sendMessage", {
      method: "POST",
      route: "/$procs/sendMessage",
      body: JSON.stringify({ args: input }),
      idempotencyKey: options?.idempotencyKey || undefined,
    });
    return {
      result: this.#decode<SendMessageResult>(answer.text, answer.status, "sendMessage"),
      idempotencyReplayed: answer.headers[IDEMPOTENCY_REPLAYED_HEADER] === "true",
    };
  }

  /** `POST /$procs/previewMessage`, sealed. */
  async previewMessage(input: PreviewInput): Promise<PreviewResult> {
    const answer = await this.#call("previewMessage", {
      method: "POST",
      route: "/$procs/previewMessage",
      body: JSON.stringify({ args: input }),
    });
    return this.#decode<PreviewResult>(answer.text, answer.status, "previewMessage");
  }

  /** `GET /messages/{id}`, sealed. */
  async getMessage(id: string): Promise<Message> {
    const answer = await this.#call(`GET /messages/${id}`, {
      method: "GET",
      route: "/messages/{id}",
      params: [id],
    });
    return this.#decode<Message>(answer.text, answer.status, `GET /messages/${id}`);
  }
}
