// @vsms/gateway — the console's data layer over the gateway's sealed transport.
//
// Everything exported here runs in the BROWSER, where the person's signing key
// is, and imports nothing from Node: a module writes an ordinary request
// (`GET /apps?limit=500`), `./upstream.ts` hands it to whatever transport the
// console installed (`./sealing-fetch.ts`: the browser's key signs it, the
// gateway's answer is believed only if it verifies), and the module reads the
// answer as JSON. No module holds a credential, a cookie or the gateway's
// address.
//
// The server half is `@vsms/gateway/bff` (`./bff/index.ts`): a pass-through of
// sealed bytes that holds no credential and no session, and the composer's own
// service key. It is a separate entry point so that nothing here can import it
// by accident and put `node:https` in a bundle for the browser.
//
// The calls below are hand-written rather than generated; see client.ts for why
// (T3, pending).

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
export type { ComposeFetchOptions, SealedProof } from "./compose-fetch";
export { createComposeFetch } from "./compose-fetch";
export type { ComposeOperation, CompositionAuthorization } from "./compose-proof";
export {
  COMPOSE_OPERATIONS,
  COMPOSE_PROOF_METHOD,
  COMPOSE_PROOF_PATH,
  composeProofBody,
  parseAuthorization,
  sha256Hex,
} from "./compose-proof";
export type { ConsoleKeyRecord } from "./console-keys";
export { getConsoleKeyByThumbprint, probeKey, revokeConsoleKey } from "./console-keys";
export type {
  DashboardSummary,
  HourlyBucket,
  OperatorDeliveryStats,
} from "./dashboard";
export { dashboardSummary } from "./dashboard";
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
  SealerFailure,
  SealingFetchOptions,
  SealingRuntime,
  WasmSealedCall,
  WasmSealer,
} from "./sealing-fetch";
export {
  createSealingFetch,
  etagOf,
  METHOD_HEADER,
  NoDeviceKeyError,
  refusalOf,
  STATUS_HEADER,
} from "./sealing-fetch";
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
export type { ComposeFetch, GatewayFetch } from "./upstream";
export {
  configureComposeFetch,
  configureGatewayFetch,
  GATEWAY_ORIGIN,
  GatewayNotConfiguredError,
} from "./upstream";
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
  updateWebhookEndpoint,
  WEBHOOK_EVENT_TYPES,
} from "./webhooks";
export type { WorkerLockInfo, WorkerLocksResult, WorkerRole } from "./workers";
export { workerLocks } from "./workers";
