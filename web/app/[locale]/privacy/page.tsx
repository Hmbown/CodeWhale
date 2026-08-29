import { permanentRedirect } from "next/navigation";

export default async function PrivacyAliasPage({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  permanentRedirect(`/${locale}/legal/privacy`);
}
