/**
 * <GettingStartedSteps> — renders the shared new-user path from
 * web/lib/content/getting-started.ts: install → first offline session →
 * provider connection → pod setup.
 *
 * Used by the homepage band and the /docs/guide page so the path reads
 * identically in both places. Server component, SSG-safe.
 */

import Link from "next/link";
import { GETTING_STARTED_STEPS } from "@/lib/content/getting-started";
import { pickText } from "@/lib/i18n/dictionaries";

export function GettingStartedSteps({ locale = "en" }: { locale?: string }) {
  return (
    // data-reveal-group: the homepage's <RevealOnScroll> settles the four
    // steps in sequence. On /docs/guide no observer is mounted and the
    // attribute is inert — the list renders complete and static, as before.
    <ol className="gs-steps" data-reveal-group>
      {GETTING_STARTED_STEPS.map((step, index) => (
        <li key={step.id} data-step-id={step.id}>
          <span className="gs-step-index" aria-hidden="true">
            {String(index + 1).padStart(2, "0")}
          </span>
          <h3>{pickText(step.title, locale)}</h3>
          <p>{pickText(step.body, locale)}</p>
          {step.commands.length > 0 && (
            <pre className="code-block gs-step-commands"><code>{step.commands.join("\n")}</code></pre>
          )}
          <Link href={`/${locale}${step.link.href}`} className="gs-step-link">
            {pickText(step.link.label, locale)} →
          </Link>
        </li>
      ))}
    </ol>
  );
}
