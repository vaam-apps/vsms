// Dumb view: the Users/Roles tab shell. The smart `UsersScreen` owns the
// `?tab=` URL state and passes the two tab bodies in as `children` slots —
// each body (`UsersTab`/`RolesTab`) is itself a smart component with its
// own data fetching, so this view only arranges them, never fetches
// anything.

import {
  ValueTabs as Tabs,
  ValueTabsContent as TabsContent,
  ValueTabsList as TabsList,
  ValueTabsTrigger as TabsTrigger,
} from "@vsms/ui";
import type { ReactNode } from "react";
import type { UsersRolesTab } from "../types";

export function UsersTabs({
  tab,
  onTabChange,
  usersPanel,
  rolesPanel,
}: {
  tab: UsersRolesTab;
  onTabChange: (tab: UsersRolesTab) => void;
  usersPanel: ReactNode;
  rolesPanel: ReactNode;
}) {
  return (
    <Tabs value={tab} onValueChange={(next) => onTabChange(next as UsersRolesTab)}>
      <TabsList>
        <TabsTrigger value="users">Users</TabsTrigger>
        <TabsTrigger value="roles">Roles</TabsTrigger>
      </TabsList>
      <TabsContent value="users">{usersPanel}</TabsContent>
      <TabsContent value="roles">{rolesPanel}</TabsContent>
    </Tabs>
  );
}
