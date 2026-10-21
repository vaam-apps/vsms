// The dashboard moved to `/`, the console's landing route. This keeps old
// bookmarks and links working.

import { redirect } from "next/navigation";

export default function DashboardRedirect() {
  redirect("/");
}
