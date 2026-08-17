import type { DocsGuideDict } from "../types";

/**
 * Polish dictionary for the docs "Getting started" page. Latin script —
 * the reference body typography is kept.
 */
export const docsGuide: DocsGuideDict = {
  metaTitle: "Pierwsze kroki · Dokumentacja Codewhale",
  metaDescription:
    "Pełna droga od instalacji do idealnej Floty: instalacja, pierwsza sesja bez kluczy, podpięcie providera i konfiguracja Floty.",
  bodyClassName: "text-ink-soft leading-relaxed",
  overviewTitle: "Pierwsze kroki",
  overviewLead:
    "Cztery kroki od jednej komendy instalacji do Floty gotowej do twojej pracy. Każdy krok opisuje wyłącznie to, co obecny kandydat faktycznie robi; wszystko niepublikowane lub niezarejestrowane jest tak oznaczone.",
  sessionTitle: "Zobacz prawdziwą sesję",
  sessionLead:
    "Poniżej jest miejsce na materiał z prawdziwej sesji. Świadomie pozostaje w stanie oczekiwania: dopóki nie powstanie nagranie dogfood z kandydata v0.9.2, ta strona nie pokazuje żadnego placeholdera ani inscenizacji.",
  nextTitle: "Co dalej",
  sourceNote:
    "Dokumenty źródłowe: docs/GUIDE.md, docs/KEYBINDINGS.md · Treść kroków żyje w web/lib/content/getting-started.ts; przy zmianie zaktualizuj docs-map.ts.",
};
