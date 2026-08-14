// Dumb — route-local to messages (R6). Fixed copy explaining the window
// `messages.list` fetched within; see `@vsms/gateway/messages.ts`'s own
// module doc for why the window exists at all.

export function TruncatedNotice() {
  return (
    <p className="text-caption text-subtle-foreground">
      Showing the most recent 1000 messages for this app — sms-api's `GET /messages` has no
      server-side filter for state or date range (see `@vsms/gateway/messages.ts`'s module doc), so
      filtering happens over that window. Older matches outside it won't appear.
    </p>
  );
}
