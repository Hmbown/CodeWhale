import Link from "next/link";
import {
  CANONICAL_MARK_SRC,
  publicAuthAppDestination,
  type PublicAuthKind,
} from "@/lib/public-auth-routes";

export function PublicAccountEntry({
  locale,
  kind,
}: {
  locale: string;
  kind: Exclude<PublicAuthKind, "callback">;
}) {
  const isZh = locale === "zh";
  const creating = kind === "sign-up";
  const appHref = publicAuthAppDestination(kind, locale);
  const otherKind = creating ? "sign-in" : "sign-up";
  const otherHref = `/${locale}/${otherKind === "sign-in" ? "signin" : "signup"}`;

  return (
    <div className="portal-home">
      <section className="portal-section">
        <div className="portal-container public-account-entry">
          {/* Pinned app-icon raster generated from brand/mark.svg; do not restyle. */}
          {/* eslint-disable-next-line @next/next/no-img-element */}
          <img
            className="public-account-mark"
            src={CANONICAL_MARK_SRC}
            alt=""
            width={64}
            height={64}
          />
          <p className="legal-doc-kicker">
            {creating
              ? isZh ? "创建账户" : "Create account"
              : isZh ? "登录" : "Sign in"}
          </p>
          <h1>
            {creating
              ? isZh ? "用一个账户同步工作。" : "Create a Codewhale account."
              : isZh ? "登录 Codewhale 账户。" : "Sign in to Codewhale."}
          </h1>
          <p className="portal-lede">
            {isZh
              ? "账户用于同步、云代理和恢复。本机开源命令行不需要账户——安装后即可在本地使用。"
              : "An account is for sync, cloud agents, and recovery. The open-source CLI still works locally without one — install it and continue on your machine."}
          </p>
          <div className="portal-actions">
            <a className="portal-button portal-button-primary" href={appHref}>
              {creating
                ? isZh ? "创建账户 →" : "Create account →"
                : isZh ? "登录 →" : "Sign in →"}
            </a>
            <Link className="portal-button portal-button-secondary" href={`/${locale}/install`}>
              {isZh ? "本机安装" : "Install locally"}
            </Link>
          </div>
          <p className="portal-meta">
            {creating
              ? isZh ? "已有账户？" : "Already have an account?"
              : isZh ? "还没有账户？" : "Need an account?"}{" "}
            <Link href={otherHref}>
              {creating
                ? isZh ? "去登录" : "Sign in"
                : isZh ? "创建账户" : "Create account"}
            </Link>
          </p>
        </div>
      </section>
    </div>
  );
}
