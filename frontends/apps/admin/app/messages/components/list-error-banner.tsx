// Dumb — route-local to messages (R6). Renders whatever error message
// `messages.list` failed with; doesn't know what the query was or why it
// failed.

import { InlineBanner } from "@vsms/ui";

export interface ListErrorBannerProps {
  message: string;
}

export function ListErrorBanner({ message }: ListErrorBannerProps) {
  return <InlineBanner variant="danger">Couldn't load messages: {message}</InlineBanner>;
}
