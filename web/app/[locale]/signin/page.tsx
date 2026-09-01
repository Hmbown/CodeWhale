import { PublicAccountEntry } from "@/components/public-account-entry";
import { buildPageMetadata } from "@/lib/page-meta";

export async function generateMetadata({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  const isZh = locale === "zh";
  return buildPageMetadata({
    path: "/signin",
    locale,
    title: isZh ? "登录 · Codewhale" : "Sign in · Codewhale",
    description: isZh
      ? "登录 Codewhale 账户以同步工作、使用云代理和恢复会话。本机开源命令行不需要账户。"
      : "Sign in to a Codewhale account for sync, cloud agents, and recovery. The local open-source CLI does not require an account.",
  });
}

export default async function SignInPage({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  return <PublicAccountEntry locale={locale} kind="sign-in" />;
}
