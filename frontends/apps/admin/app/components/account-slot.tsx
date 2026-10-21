// Dumb component (R6): the account block `SideNav` renders under its footer
// rows from 1280px and inside its More sheet below that: who is signed in,
// the theme preference (including "follow the system", which the top bar's
// one-click toggle cannot express), and sign out.
//
// No hard-coded `id` in here: `SideNav` can mount this block twice at once
// (the hidden sidebar copy and the sheet's copy). No `Dialog` either: inside
// the sheet a dialog would open under the sheet's scrim. The theme picker is
// a `SelectDropdown` rather than the library's `ThemeSwitcher` radio cards,
// which are three tall rows in a 288px footer. Sign out is a plain form
// post, the same request the console has always made.

import {
  Button,
  Select,
  SelectDropdown,
  SelectItem,
  SelectTrigger,
  SelectValue,
  type ThemePreference,
} from "@vaam-apps/ui";
import { LogOut, UserRound } from "lucide-react";

export interface AccountSlotProps {
  /** The signed-in human's email, or absent when the session has none
   * (the OP's `id_token` carries no `email` claim for every account). */
  email?: string | null | undefined;
  themePreference: ThemePreference;
  onThemePreferenceChange: (next: ThemePreference) => void;
}

function isThemePreference(value: string): value is ThemePreference {
  return value === "system" || value === "light" || value === "dark";
}

export function AccountSlot({ email, themePreference, onThemePreferenceChange }: AccountSlotProps) {
  const shown = email != null && email !== "" ? email : undefined;
  return (
    <div className="flex flex-col gap-3 py-1">
      <div className="flex min-w-0 items-center gap-3">
        <span
          aria-hidden="true"
          className="flex size-10 shrink-0 items-center justify-center rounded-full bg-primary-container font-medium text-on-primary-container text-title-medium"
        >
          {shown?.[0]?.toUpperCase() ?? <UserRound size={20} />}
        </span>
        <div className="flex min-w-0 flex-1 flex-col">
          <span className="text-label-medium text-muted-foreground">Signed in</span>
          {shown !== undefined && <span className="truncate text-body-medium">{shown}</span>}
        </div>
        <form action="/api/auth/logout" method="post">
          <Button type="submit" variant="ghost" size="icon" aria-label="Sign out" title="Sign out">
            <LogOut size={18} aria-hidden="true" />
          </Button>
        </form>
      </div>
      <div className="flex items-center justify-between gap-3">
        <span className="text-label-large text-muted-foreground">Appearance</span>
        <Select
          value={themePreference}
          onValueChange={(next) => {
            if (isThemePreference(next)) onThemePreferenceChange(next);
          }}
        >
          <SelectTrigger aria-label="Appearance" className="w-32">
            <SelectValue />
          </SelectTrigger>
          <SelectDropdown>
            <SelectItem value="system">System</SelectItem>
            <SelectItem value="light">Light</SelectItem>
            <SelectItem value="dark">Dark</SelectItem>
          </SelectDropdown>
        </Select>
      </div>
    </div>
  );
}
