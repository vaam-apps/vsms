// Server component shell (#50), same shape `workers/page.tsx` uses: no
// `useSearchParams`/`nuqs` on this screen, so no Suspense boundary is
// needed around it.

import { MessageDetailScreen } from "./message-detail-screen";

export default async function MessageDetailPage({ params }: { params: Promise<{ id: string }> }) {
  const { id } = await params;
  return <MessageDetailScreen messageId={id} />;
}
