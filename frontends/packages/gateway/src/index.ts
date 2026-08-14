import "server-only";

// @vsms/gateway — server-only mTLS + Bearer seam to sms-api. See
// client.ts's module doc for why the two calls below are hand-written
// rather than generated (T3, pending).

export type {
  AppClientListItem,
  AppClientRecord,
  ProvisionClientResult,
} from "./app-clients";
export {
  getAppClientById,
  listAppClientsForApp,
  provisionClient,
  retireAppClient,
  unpackScopes,
  updateAppClient,
} from "./app-clients";
export type {
  AppListItem,
  AppRecord,
  CreateAppFields,
  UpdateAppFields,
} from "./apps";
export {
  createApp,
  deleteApp,
  getAppById,
  listApps,
  packIpAllowlist,
  unpackIpAllowlist,
  updateApp,
} from "./apps";
export type {
  AuditChainStatus,
  AuditLogEntry,
  AuditLogPage,
  AuditLogQuery,
} from "./audit-log";
export { fetchAuditChainStatus, fetchAuditLog } from "./audit-log";
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
export type {
  DashboardSummary,
  HourlyBucket,
  OperatorDeliveryStats,
} from "./dashboard";
export { dashboardSummary } from "./dashboard";
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
  OptOutListItem,
  OptOutRecord,
  OptOutSource,
  OptOutSummary,
  RecordOptOutFields,
  SearchOptOutResult,
} from "./opt-outs";
export {
  deleteOptOut,
  listOptOuts,
  recordOptOut,
  searchOptOutByMsisdn,
} from "./opt-outs";
export type {
  ProviderKind,
  ProviderListItem,
  ProviderRecord,
  ProviderState,
  UpdateProviderFields,
} from "./providers";
export { getProviderById, listProviders, updateProvider } from "./providers";
export type { RequestCredential } from "./request-credential";
export { runWithRequestCredential } from "./request-credential";
export type { WithEtag } from "./rest";
export { deleteResource, fetchWithEtag, postJson, updateWithIfMatch } from "./rest";
export type {
  CreateRoleFields,
  RoleRecord,
  UpdateRoleFields,
} from "./roles";
export {
  createRole,
  deleteRole,
  getRoleById,
  isReservedRoleKey,
  isValidRoleKeyShape,
  listRoles,
  packPermissions,
  unpackPermissions,
  updateRole,
} from "./roles";
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
export type {
  CreateSenderIdFields,
  CreateSenderIdRegistrationFields,
  SenderIdRecord,
  SenderIdRegistrationRecord,
  UpdateSenderIdFields,
  UpdateSenderIdRegistrationFields,
} from "./senders";
export {
  createSenderId,
  createSenderIdRegistration,
  getSenderIdById,
  listSenderIdRegistrations,
  listSenderIds,
  updateSenderId,
  updateSenderIdRegistration,
} from "./senders";
export { getMachineAccessToken, invalidateMachineAccessToken } from "./token";
export type {
  ProvisionUserResult,
  UpdateUserFields,
  UserListItem,
  UserRecord,
} from "./users";
export {
  deleteUser,
  getUserById,
  listUsers,
  provisionUser,
  updateUser,
} from "./users";
export type {
  AttemptState,
  CreatedWebhookEndpoint,
  CreateWebhookEndpointFields,
  ListWebhookAttemptsInput,
  ListWebhookAttemptsResult,
  UpdateWebhookEndpointFields,
  WebhookAttemptRecord,
  WebhookEndpointRecord,
  WebhookEventType,
} from "./webhooks";
export {
  ATTEMPT_STATES,
  createWebhookEndpoint,
  deleteWebhookEndpoint,
  getWebhookEndpointById,
  listWebhookAttempts,
  listWebhookEndpoints,
  replayWebhookAttempt,
  rotateWebhookSecret,
  updateWebhookEndpoint,
  WEBHOOK_EVENT_TYPES,
} from "./webhooks";
export type { WorkerLockInfo, WorkerLocksResult, WorkerRole } from "./workers";
export { workerLocks } from "./workers";
