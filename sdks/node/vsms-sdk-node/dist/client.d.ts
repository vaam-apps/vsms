import { type PrivateKeyJwtConfigOptions, type TokenStore } from "./token.js";
import type { Message, PreviewInput, PreviewResult, SendMessageInput, SendMessageResult } from "./types.js";
export interface SendMessageOptions {
    idempotencyKey?: string | undefined;
}
export interface SendMessageOutcome {
    result: SendMessageResult;
    idempotencyReplayed: boolean;
}
export interface VsmsClientOptions {
    baseUrl: string;
    tokenStore: TokenStore;
}
export interface PrivateKeyJwtClientOptions extends PrivateKeyJwtConfigOptions {
    keyPath?: string | undefined;
    privateKeyPem?: string | undefined;
}
export declare class VsmsClient {
    #private;
    constructor(options: VsmsClientOptions);
    static privateKeyJwt(options: PrivateKeyJwtClientOptions): VsmsClient;
    get tokenStore(): TokenStore;
    /**
     * `POST /$procs/sendMessage`. Handles token acquisition, caching, bounded 401 retry,
     * and optional `Idempotency-Key` header with replay tracking.
     */
    sendMessage(input: SendMessageInput, options?: SendMessageOptions | undefined): Promise<SendMessageOutcome>;
    /**
     * `POST /$procs/previewMessage`.
     */
    previewMessage(input: PreviewInput): Promise<PreviewResult>;
    /**
     * `GET /messages/{id}`.
     */
    getMessage(id: string): Promise<Message>;
}
//# sourceMappingURL=client.d.ts.map