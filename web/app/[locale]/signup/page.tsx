import { PublicAccountEntry } from "@/components/public-account-entry";
import { buildPageMetadata } from "@/lib/page-meta";

export async function generateMetadata({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  const isZh = locale === "zh";
  return buildPageMetadata({
    path: "/signup",
    locale,
    title: isZh ? "创建账户 · Codewhale" : "Create account · Codewhale",
    description: isZh
      ? "创建 Codewhale 账户以同步工作、使用云代理和恢复会话。本机开源命令行不需要账户。"
      : "Create a Codewhale account for sync, cloud agents, and recovery. The local open-source CLI does not require an account.",
  });
}

export default async function SignUpPage({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  return <PublicAccountEntry locale={locale} kind="sign-up" />;
}
