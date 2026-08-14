// Dumb component (R6): the sidebar footer's "Signed in as <email> · Sign
// out" block (console-redesign.md §4). Markup moved verbatim out of
// `console-shell.tsx` — that file built this JSX inline as a local
// `accountSlot` variable before handing it to `SideNav`'s own `accountSlot`
// prop; the value itself is unchanged, only where it's defined.

export interface AccountSlotProps {
  /** The signed-in human's email, or `null`/absent when there is none to
   * show — see `console-shell.tsx`'s own doc on where this comes from. */
  email?: string | null | undefined;
}

export function AccountSlot({ email }: AccountSlotProps) {
  return (
    <div className="flex flex-col gap-1 text-caption text-subtle-foreground">
      {email != null && email !== "" && (
        <p className="truncate">
          Signed in as <span className="text-muted-foreground">{email}</span>
        </p>
      )}
      <form action="/api/auth/logout" method="post">
        <button
          type="submit"
          className="text-muted-foreground underline decoration-edge-strong underline-offset-2 hover:text-foreground hover:decoration-foreground"
        >
          Sign out
        </button>
      </form>
    </div>
  );
}
