// Dumb — route-local to messages (R6). Renders whatever error message
// `messages.list` failed with; doesn't know what the query was or why it
// failed.

export interface ListErrorBannerProps {
  message: string;
}

export function ListErrorBanner({ message }: ListErrorBannerProps) {
  return (
    <div className="rounded-sm border border-state-danger-border bg-state-danger-bg px-3 py-2 text-caption text-state-danger-fg">
      Couldn't load messages: {message}
    </div>
  );
}
