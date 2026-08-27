import { redirect } from "next/navigation";

export default async function LegalIndexPage({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  redirect(`/${locale}/legal/terms`);
}
