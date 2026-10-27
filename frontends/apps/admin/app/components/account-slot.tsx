// Dumb component (R6): the account block `SideNav` renders under its footer
// rows from 1280px and inside its More sheet below that: which key this console
// signs with and how long it has left, the way to its lifecycle screen, and the
// theme preference (including "follow the system", which the top bar's
// one-click toggle cannot express).
//
// No hard-coded `id` in here: `SideNav` can mount this block twice at once
// (the hidden sidebar copy and the sheet's copy). No `Dialog` either: inside
// the sheet a dialog would open under the sheet's scrim. The theme choice is
// three buttons, not a menu: a choice among three is shown, not hidden behind a
// control that opens (the same rule that keeps dropdowns out of the product).

import { Button, type ThemePreference } from "@vaam-apps/ui";
import { KeyRound } from "lucide-react";
import Link from "next/link";

export interface AccountSlotProps {
  /** One line about the key: "Paired, 12 days left", or what is wrong. */
  keySummary: string;
  /** The person the key speaks as, when the console has been able to read it. */
  person?: string | undefined;
  themePreference: ThemePreference;
  onThemePreferenceChange: (next: ThemePreference) => void;
}

const THEMES: { value: ThemePreference; label: string }[] = [
  { value: "system", label: "System" },
  { value: "light", label: "Light" },
  { value: "dark", label: "Dark" },
];

export function AccountSlot({
  keySummary,
  person,
  themePreference,
  onThemePreferenceChange,
}: AccountSlotProps) {
  return (
    <div className="flex flex-col gap-3 py-1">
      <Link
        href="/console"
        className="flex min-w-0 items-center gap-3 rounded-xl focus-visible:outline-2 focus-visible:outline-primary"
      >
        <span
          aria-hidden="true"
          className="flex size-10 shrink-0 items-center justify-center rounded-full bg-primary-container text-on-primary-container"
        >
          <KeyRound size={20} />
        </span>
        <span className="flex min-w-0 flex-1 flex-col">
          <span className="text-label-medium text-muted-foreground">This console</span>
          {person !== undefined && <span className="truncate text-body-medium">{person}</span>}
          <span className="truncate text-body-small text-muted-foreground">{keySummary}</span>
        </span>
      </Link>
      <div className="flex items-center justify-between gap-3">
        <span className="text-label-large text-muted-foreground">Appearance</span>
        <fieldset className="m-0 flex gap-1 border-0 p-0">
          <legend className="sr-only">Appearance</legend>
          {THEMES.map(({ value, label }) => (
            <Button
              key={value}
              type="button"
              size="sm"
              variant={themePreference === value ? "primary" : "ghost"}
              aria-pressed={themePreference === value}
              onClick={() => onThemePreferenceChange(value)}
            >
              {label}
            </Button>
          ))}
        </fieldset>
      </div>
    </div>
  );
}
