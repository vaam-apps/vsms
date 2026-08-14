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
export interface TokenStore {
    get(): Promise<string>;
    invalidate(): Promise<void> | void;
}
export declare class PrivateKeyJwtTokenStore implements TokenStore {
    #private;
    constructor(options: PrivateKeyJwtConfigOptions, keyMaterial: {
        pemString?: string | undefined;
        keyPath?: string | undefined;
    });
    static fromKeyPath(issuer: string, clientId: string, keyPath: string, scope?: string | undefined, audience?: TokenAudience | undefined): PrivateKeyJwtTokenStore;
    static fromKeyPem(issuer: string, clientId: string, pemString: string, scope?: string | undefined, audience?: TokenAudience | undefined): PrivateKeyJwtTokenStore;
    get(): Promise<string>;
    invalidate(): void;
}
//# sourceMappingURL=token.d.ts.map