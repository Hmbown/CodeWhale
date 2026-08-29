import { permanentRedirect } from "next/navigation";

export default async function LegalIndexPage({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  permanentRedirect(`/${locale}/legal/terms`);
}
