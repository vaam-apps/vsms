import "server-only";

// @vsms/gateway — server-only mTLS + Bearer seam to sms-api. See
// client.ts's module doc for why the two calls below are hand-written
// rather than generated (T3, pending).

export type {
  Encoding,
  MessageClass,
  MessageState,
  OperatorCode,
  PreviewInput,
  PreviewResult,
  SendMessageInput,
  SendMessageResult,
} from "./client";
export { previewMessage, sendMessage } from "./client";
export { gatewayAgent } from "./dispatcher";
export type { GatewayFieldErrors, GatewayTrpcCode } from "./errors";
export { GatewayError, mapGatewayError } from "./errors";
export type {
  MessageStateEvent,
  MessageStreamDegradedEvent,
  MessageStreamFilter,
  MessageStreamFrame,
  MessageStreamHubOptions,
  MessageStreamRecoveredEvent,
} from "./message-stream";
export {
  getMessageStreamHub,
  MessageStreamHub,
} from "./message-stream";
export type {
  ListMessagesInput,
  ListMessagesResult,
  MessageListItem,
  MessageRecord,
  StreamCandidate,
} from "./messages";
export { getMessageById, listMessages, listMessagesForStream } from "./messages";
export { getAccessToken, invalidateAccessToken } from "./token";
