import Link from "next/link";
import { APP_LOGIN_URL } from "@/lib/i18n/links";
import { buildPageMetadata } from "@/lib/page-meta";

export async function generateMetadata({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  const isZh = locale === "zh";
  return buildPageMetadata({
    path: "/pricing",
    locale,
    title: isZh ? "价格 · Codewhale" : "Pricing · Codewhale",
    description: isZh
      ? "Codewhale 开源运行时免费。托管会员计费已实现但尚未对公众开放。"
      : "The Codewhale open-source runtime is free. Hosted membership billing is built but not for sale yet.",
  });
}

export default async function PricingPage({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  const isZh = locale === "zh";
  return (
    <div className="portal-home">
      <section className="portal-section">
        <div className="portal-container pricing-page">
          <p className="legal-doc-kicker">{isZh ? "价格" : "Pricing"}</p>
          <h1>{isZh ? "现在没有可以付款的套餐。" : "There is nothing to pay for yet."}</h1>
          <p className="portal-lede">
            {isZh
              ? "开源运行时免费，自带模型密钥（BYOK）。托管计算的 $10/月会员已经写进产品，但生产环境的计费开关仍是休眠状态——没有结账按钮，也就不会误收一笔钱。"
              : "The open-source runtime is free. Bring your own model key. The $10/month Member plan for hosted compute is implemented in the product, but production billing is dormant — so there is no checkout button that could charge anyone by accident."}
          </p>
          <ul className="pricing-list">
            <li>
              <strong>{isZh ? "开源 / 自托管" : "Open source / self-hosted"}</strong>
              <span>{isZh ? "免费。在你的机器上运行，使用你自己的模型密钥。" : "Free. Run on your machine with your own model keys."}</span>
            </li>
            <li>
              <strong>{isZh ? "托管会员" : "Hosted Member"}</strong>
              <span>
                {isZh
                  ? "$10/月、每期 $10 可结转的托管计算额度——已实现，尚未对公众销售。"
                  : "$10/month with $10 of rollover hosted-compute credits each paid period — built, not for sale to the public."}
              </span>
            </li>
          </ul>
          <p className="pricing-note">
            {isZh
              ? "模型用量由你连接的提供商计费，Codewhale 不加价。我们不会在计费休眠期间展示一个会失败的“立即购买”。"
              : "Model tokens are billed by the provider you connect. Codewhale does not mark them up. We will not show a Buy button that 503s while billing is dormant."}
          </p>
          <div className="portal-actions">
            <Link className="portal-button portal-button-primary" href={`/${locale}/install`}>
              {isZh ? "安装 →" : "Install →"}
            </Link>
            <a className="portal-button portal-button-secondary" href={APP_LOGIN_URL}>
              {isZh ? "打开应用" : "Open the app"}
            </a>
          </div>
        </div>
      </section>
    </div>
  );
}
