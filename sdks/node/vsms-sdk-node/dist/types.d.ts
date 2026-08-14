/**
 * Types describing vsms domain models and procedures.
 */
export type Encoding = "gsm7" | "ucs2";
export type OperatorCode = "mtn" | "orange" | "camtel" | "nexttel" | "unknown";
export type MessageClass = "otp" | "transactional" | "notification" | "marketing";
export type MessageState = "accepted" | "queued" | "routed" | "submitted" | "delivered" | "uncertain" | "undelivered" | "failed" | "expired" | "rejected" | "cancelled";
export interface PreviewInput {
    body: string;
    to?: string;
}
export interface PreviewResult {
    encoding: Encoding;
    segments: number;
    length: number;
    perSegment: number;
    offending: string[];
    suggestion?: string;
    operator: OperatorCode;
    normalizedTo?: string;
}
export interface SendMessageInput {
    to: string;
    body: string;
    senderId?: string;
    class?: MessageClass;
    clientRef?: string;
    /** ISO-8601 string */
    scheduledAt?: string;
    validityMinutes?: number;
}
export interface SendMessageResult {
    messageId: string;
    state: MessageState;
    encoding: Encoding;
    segments: number;
    operator: OperatorCode;
    estimatedCostXaf: string;
}
export interface Message {
    id: string;
    state: MessageState;
    to: string;
    body: string;
    senderId?: string;
    class: MessageClass;
    clientRef?: string;
    encoding: Encoding;
    segments: number;
    operator: OperatorCode;
    estimatedCostXaf: string;
    providerMessageRef?: string;
    createdAt: string;
    updatedAt: string;
    [key: string]: unknown;
}
//# sourceMappingURL=types.d.ts.map