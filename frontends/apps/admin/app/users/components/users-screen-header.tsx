// Dumb view: the screen title, above the tabs.

export function UsersScreenHeader() {
  return (
    <div className="border-edge border-b pb-6">
      <h1 className="font-medium text-foreground text-title">Users &amp; roles</h1>
      <p className="mt-1 max-w-xl text-body text-muted-foreground">
        Console accounts and the permission sets their roles carry.
      </p>
    </div>
  );
}
