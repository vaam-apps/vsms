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
export { GatewayError, isStaleWriteError, mapGatewayError } from "./errors";
export type {
  JobListItem,
  JobRecord,
  JobState,
  ListJobsInput,
  ListJobsResult,
  RequeueJobResult,
} from "./jobs";
export { listJobs, requeueJob } from "./jobs";
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
  DeliveryOutcome,
  DeliveryReceiptSummary,
  ListMessagesInput,
  ListMessagesResult,
  MessageListItem,
  MessageReceiptsResult,
  MessageRecord,
  StreamCandidate,
} from "./messages";
export {
  getMessageById,
  listMessageReceipts,
  listMessages,
  listMessagesForStream,
} from "./messages";
export type {
  ProviderKind,
  ProviderListItem,
  ProviderRecord,
  ProviderState,
  UpdateProviderFields,
} from "./providers";
export { getProviderById, listProviders, updateProvider } from "./providers";
export type { WithEtag } from "./rest";
export { deleteResource, fetchWithEtag, postJson, updateWithIfMatch } from "./rest";
export type {
  PredicateKind,
  RouteEvaluationInfo,
  RouteOutcomeKind,
  RouteWinnerInfo,
  SimulateMessageClass,
  SimulateOperatorCode,
  SimulateRouteInput,
  SimulateRouteResult,
  TieBreakInfo,
  TieBreakRangeInfo,
} from "./route-simulator";
export { simulateRoute } from "./route-simulator";
export type {
  CreateRouteFields,
  RouteListItem,
  RouteMessageClass,
  RouteOperatorCode,
  RouteRecord,
  UpdateRouteFields,
} from "./routes";
export { createRoute, deleteRoute, getRouteById, listRoutes, updateRoute } from "./routes";
export { getAccessToken, invalidateAccessToken } from "./token";
export type { WorkerLockInfo, WorkerLocksResult, WorkerRole } from "./workers";
export { workerLocks } from "./workers";
