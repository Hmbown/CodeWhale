import { redirect } from "next/navigation";
import { APP_AUTH_CALLBACK_URL } from "@/lib/public-auth-routes";

/**
 * Fallback if a localized `/<locale>/auth/callback` reaches the page
 * renderer. Middleware already hops this to the CWC app; keep the same
 * destination so a missed middleware match cannot 404.
 */
export default async function PublicAuthCallbackPage({
  searchParams,
}: {
  searchParams: Promise<Record<string, string | string[] | undefined>>;
}) {
  const params = await searchParams;
  const destination = new URL(APP_AUTH_CALLBACK_URL);
  for (const [key, value] of Object.entries(params)) {
    if (typeof value === "string") destination.searchParams.set(key, value);
    else if (Array.isArray(value)) {
      for (const item of value) destination.searchParams.append(key, item);
    }
  }
  redirect(destination.toString());
}
