import Link from "next/link";
import { PUBLIC_MEMBERSHIP_COPY } from "@/lib/content/membership";
import { pickText } from "@/lib/i18n/dictionaries";
import { APP_LOGIN_URL, APP_SIGNUP_URL } from "@/lib/i18n/links";
import { buildPageMetadata } from "@/lib/page-meta";

export async function generateMetadata({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  return buildPageMetadata({
    path: "/pricing",
    locale,
    title: pickText(PUBLIC_MEMBERSHIP_COPY.metadata.title, locale),
    description: pickText(PUBLIC_MEMBERSHIP_COPY.metadata.description, locale),
  });
}

export default async function PricingPage({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  return (
    <div className="portal-home">
      <section className="portal-section">
        <div className="portal-container pricing-page">
          <p className="legal-doc-kicker">{pickText(PUBLIC_MEMBERSHIP_COPY.kicker, locale)}</p>
          <h1>{pickText(PUBLIC_MEMBERSHIP_COPY.title, locale)}</h1>
          <p className="portal-lede">
            {pickText(PUBLIC_MEMBERSHIP_COPY.lead, locale)}
          </p>
          <ul className="pricing-list">
            {PUBLIC_MEMBERSHIP_COPY.options.map((option) => (
              <li key={option.id}>
                <strong>{pickText(option.title, locale)}</strong>
                <span>{pickText(option.body, locale)}</span>
              </li>
            ))}
          </ul>
          <p className="pricing-note">
            {pickText(PUBLIC_MEMBERSHIP_COPY.note, locale)}
          </p>
          <div className="portal-actions">
            <a className="portal-button portal-button-primary" href={APP_SIGNUP_URL}>
              {pickText(PUBLIC_MEMBERSHIP_COPY.actions.createAccount, locale)}
            </a>
            <Link className="portal-button portal-button-secondary" href={`/${locale}/install`}>
              {pickText(PUBLIC_MEMBERSHIP_COPY.actions.continueLocally, locale)}
            </Link>
          </div>
          <p className="pricing-note">
            <a href={APP_LOGIN_URL} className="body-link">{pickText(PUBLIC_MEMBERSHIP_COPY.actions.signIn, locale)}</a>
          </p>
        </div>
      </section>
    </div>
  );
}
