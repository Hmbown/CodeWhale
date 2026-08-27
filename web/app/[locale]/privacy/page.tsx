import { redirect } from "next/navigation";

export default async function PrivacyAliasPage({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  redirect(`/${locale}/legal/privacy`);
}
