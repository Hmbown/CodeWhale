import type { HomeDict } from "../types";

/**
 * Polish home dictionary — strona startowa „gazeta-ocean".
 *
 * Natywny przekład w aktualnym kierunku angielskiego: przynieś własny
 * model, wszystko dzieje się na twojej maszynie. Słownictwo produktu
 * pozostaje dosłowne jak w pakiecie TUI: Plan / Work / Operate,
 * Ask / Auto-Review / Full Access, Codewhale, TUI, `codewhale exec`,
 * Runtime API + MCP, fleet, Node 18+, Rust, MIT.
 *
 * Pieczęcie sekcji (法, 行, …) to glify współdzielone z wydaniem
 * angielskim — znaki, nie proza.
 */
export const home: HomeDict = {
  metaTitle: "Codewhale — zanurza się w głębinach, żebyś ty nie musiał.",
  metaDescription:
    "Codewhale zanurza się w głębinach, żebyś ty nie musiał — otwartoźródłowy agent do kodowania w terminalu. Przynieś własny model. Działa na twojej maszynie. Rust, MIT.",

  kicker: "Open source · Własny model · Działa w terminalu",
  heroTitleA: "Codewhale zanurza się w głębinach,",
  heroTitleB: "żebyś ty nie musiał.",
  heroIntro:
    "{brand} to otwartoźródłowy agent do kodowania w twoim terminalu. Daj mu model i zadanie — przeczyta twój kod, zedytuje pliki, uruchomi własne sprawdzenia i zatrzyma się, gdy praca jest skończona albo gdy potrzebuje ciebie. Przynieś dowolny model albo je mieszaj: przypnij inny model do każdej roli.",
  install: "Instaluj",
  docs: "Dokumentacja",
  copy: "Kopiuj",
  copied: "Skopiowano ✓",

  installEyebrow: "instalacja jedną komendą",
  installRequirement: "wymaga Node 18+ — bez toolchaina Rust",
  installOtherWays: "inne sposoby →",

  latestRelease: "Najnowsze wydanie {tag}",
  releaseUnavailable: "Status wydania niedostępny",
  currentSource: "Źródło",
  sourceCandidate: "Niewydane",
  providerRoutes: "{count} providerów",
  publishedRelease: "wydane",
  figcaptionSourceCandidate: "niewydane",

  shotSession: "Bieżąca sesja",
  screenshotAlt:
    "Bieżąca sesja Codewhale w terminalu: tryb Operate, wieloryb, kompozytor i stopka",
  figcaption: "Bieżąca sesja Codewhale · tryb Operate · postawa uprawnień Ask",

  proofHeading: "Podwodna powłoka terminala. Dowolny model. Na twojej maszynie.",
  proofBody:
    "Przynieś model, którego już używasz — hostowany, przez gateway albo lokalny. Plan / Work / Operate i jawne postawy uprawnień trzymają nurkowanie pod twoją kontrolą.",

  sealDecides: "法",
  decidesEyebrow: "Zobacz, jak decyduje",
  decidesHeading: "Prawo widoczne w śladzie",
  decidesLede:
    "Fragmenty prawdziwych sesji — rozstrzygnięta hierarchia reguł projektu widać w rozumowaniu modelu, to nie tylko deklaracja ze strony startowej.",

  sealWorkflow: "行",
  workflowHeading: "Od zadania do zweryfikowanej zmiany.",
  workflow: [
    ["Zbadaj", "Przeczytaj repozytorium, jego instrukcje i zadanie."],
    ["Zadziałaj", "Edytuj pliki w jawnych granicach zatwierdzeń."],
    ["Zweryfikuj", "Uruchom sprawdzenia i obejrzyj wynik."],
    ["Zrelacjonuj", "Zostaw zwięzłe, trwałe potwierdzenie."],
  ],
  receiptAria: "Przykładowe potwierdzenie pracy",
  receiptInspect: "repozytorium i instrukcje",
  receiptAct: "edycja przez wybraną postawę uprawnień",
  receiptReport: "sprawdzenia zaliczone · potwierdzenie zapisane",

  sealStart: "起",
  startHeading: "Nowy w Codewhale? Cztery kroki od początku do końca.",
  startLede:
    "Instalacja → pierwsza sesja bez kluczy → podpięcie providera → konfiguracja Floty. Pojęcia są zdefiniowane na stronie słownika.",
  startGuideLink: "Przeczytaj przewodnik na start →",
  startVocabularyLink: "Zobacz słownik produktu →",

  sealBoundaries: "界",
  boundariesHeadingA: "Twój model.",
  boundariesHeadingB: "Twoje granice.",
  boundariesBody:
    "Wybierz model, tryb pracy i postawę uprawnień jawnie. Nieznany koszt pozostaje zadeklarowany jako nieznany, a powierzchnie poglądowe są tak oznaczone.",
  hostedGatewayLocal: "Modele hostowane, przez gateway i lokalne",
  planActOperateDesc: "Od planowania tylko do odczytu po autonomiczną operację",
  askAutoReviewDesc: "Wybierz postawę uprawnień do pracy",
  tuiExecWebDesc: "Interaktywne i bezgłowowe powierzchnie runtime'u",

  sealSurfaces: "面",
  surfacesHeading: "Używaj runtime'u tam, gdzie odbywa się praca.",
  surfaces: [
    ["TUI", "Interaktywna praca w terminalu"],
    ["codewhale exec", "Skrypty i CI"],
    ["Klient web", "Klient przeglądarkowy, tylko loopback"],
    ["Runtime API + MCP", "Lokalne integracje"],
    ["fleet", "Trwała praca wielu agentów"],
  ],
  runtimeLink: "Zobacz powierzchnie runtime'u i notatki o stabilności →",

  installBandHeading: "Zacznij jedną komendą.",
  binaries: "Binarki",
  chinaMirrors: "Mirrory w Chinach",
  installGuideLink: "Przeczytaj przewodnik instalacji →",

  sealCommunity: "众",
  communityHeading: "Budowane jawnie",
  communityBody:
    "Na licencji MIT, kształtowane przez współtwórców od runtime'ów, przez providerów, platformy, dokumentację po testy.",
  communityLinksAria: "Linki społeczności",
  contribute: "Współtwórz",
};
