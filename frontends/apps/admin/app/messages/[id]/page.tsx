// Server component shell (#50), same shape `workers/page.tsx` uses: no
// `useSearchParams`/`nuqs` on this screen, so no Suspense boundary is
// needed around it.
//
// R6 ("Pages validate their inputs"): `id` is validated against the same
// `Cuid` shape the server itself enforces (`message-id.ts`) before it ever
// reaches a component — a malformed id is a 404 answered here, not a
// screen rendered around a request that could never have succeeded.

import { notFound } from "next/navigation";
import { MessageDetailScreen } from "./message-detail-screen";
import { isValidMessageId } from "./message-id";

export default async function MessageDetailPage({ params }: { params: Promise<{ id: string }> }) {
  const { id } = await params;
  if (!isValidMessageId(id)) {
    notFound();
  }
  return <MessageDetailScreen messageId={id} />;
}
