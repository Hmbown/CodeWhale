import type { HomeDict } from "../types";

/**
 * Ukrainian home pack — native rewrite mirroring the current English copy:
 * bring-your-own-model positioning, no trace of the old "local-first" or
 * "LLM leverage for ordinary people" lines. Established terminology stays:
 * "receipt" renders as «протокол» (a durable, official record) so it stays
 * distinct from the workflow step «Звіт» (Report); "permission posture" is
 * «режим дозволів»; "trace" is «хід міркувань». Mode and permission names —
 * Plan / Work / Operate, Ask / Auto-Review / Full Access — stay literal,
 * matching crates/tui/locales/uk.json.
 */
export const home: HomeDict = {
  metaTitle: "Codewhale — занурюється в глибину, щоб не довелося вам.",
  metaDescription:
    "Codewhale занурюється в глибину, щоб не довелося вам — термінальний агент із відкритим кодом, що пише код. Підключіть власну модель. Працює на вашій машині. Rust, MIT.",

  kicker: "Відкритий код · Ваша власна модель · Працює у вашому терміналі",
  heroTitleA: "Codewhale занурюється в глибину,",
  heroTitleB: "щоб не довелося вам.",
  heroIntro:
    "{brand} — агент із відкритим кодом, що пише код у вашому терміналі. Дайте йому модель і завдання — він прочитає ваш код, редагує файли, сам запустить перевірки й зупиниться, коли роботу зроблено або коли йому потрібні ви. Підходить будь-яка модель, а можна й змішувати: закріпіть окрему модель за кожною роллю.",
  install: "Встановити",
  docs: "Документація",
  copy: "Копіювати",
  copied: "Скопійовано ✓",

  installEyebrow: "встановлення одним рядком",
  installRequirement: "потрібен Node 18+ — без тулчейна Rust",
  installOtherWays: "інші способи →",

  latestRelease: "Останній реліз {tag}",
  releaseUnavailable: "Статус релізу недоступний",
  currentSource: "Джерело",
  sourceCandidate: "Не випущено",
  providerRoutes: "Провайдерів: {count}",
  publishedRelease: "випущено",
  figcaptionSourceCandidate: "не випущено",

  shotSession: "Поточний сеанс",
  screenshotAlt:
    "Поточний термінальний сеанс Codewhale: режим Operate, кит, композер і нижня панель",
  figcaption: "Поточний сеанс Codewhale · режим Operate · дозволи Ask",

  proofHeading: "Підводна термінальна оболонка. Будь-яка модель. На вашій машині.",
  proofBody:
    "Підключіть модель, якою вже користуєтеся — хмарну, шлюзову чи локальну. Режими Plan / Work / Operate та явні режими дозволів тримають занурення під вашим контролем.",

  sealDecides: "法",
  decidesEyebrow: "Подивіться, як він ухвалює рішення",
  decidesHeading: "Правила, які видно в ході міркувань",
  decidesLede:
    "Фрагменти справжнього сеансу — ранжовані правила проєкту видно в міркуваннях моделі, а не лише в заяві на сторінці.",

  sealWorkflow: "行",
  workflowHeading: "Від завдання до перевіреної зміни.",
  workflow: [
    ["Огляд", "Читає репозиторій, його інструкції та завдання."],
    ["Дія", "Редагує файли в явно окреслених межах схвалення."],
    ["Перевірка", "Запускає перевірки та вивчає результат."],
    ["Звіт", "Залишає стислий, довговічний протокол."],
  ],
  receiptAria: "Приклад робочого протоколу",
  receiptInspect: "репозиторій та інструкції",
  receiptAct: "редагування в межах обраного режиму дозволів",
  receiptReport: "перевірки пройдено · протокол збережено",

  sealStart: "起",
  startHeading: "Уперше в Codewhale? Чотири кроки від початку до кінця.",
  startLede:
    "Встановлення → перший сеанс без ключів → підключення провайдера → перший робочий процес у fleet. Терміни пояснено на сторінці словника.",
  startGuideLink: "Читати посібник для початківців →",
  startVocabularyLink: "Переглянути словник продукту →",

  sealBoundaries: "界",
  boundariesHeadingA: "Ваша модель.",
  boundariesHeadingB: "Ваші межі.",
  boundariesBody:
    "Явно обирайте модель, режим роботи та режим дозволів. Невідома вартість лишається невідомою, а інтерфейси зі статусом попереднього перегляду позначені саме так.",
  hostedGatewayLocal: "Хмарні, шлюзові та локальні моделі",
  planActOperateDesc: "Від планування лише для читання до автономного виконання",
  askAutoReviewDesc: "Оберіть режим дозволів для роботи",
  tuiExecWebDesc: "Інтерактивні та неінтерактивні інтерфейси рантайму",

  sealSurfaces: "面",
  surfacesHeading: "Використовуйте рантайм там, де відбувається робота.",
  surfaces: [
    ["TUI", "Інтерактивна робота в терміналі"],
    ["codewhale exec", "Скрипти та CI"],
    ["Вебклієнт", "Браузерний клієнт лише через loopback"],
    ["Runtime API + MCP", "Локальні інтеграції"],
    ["fleet", "Стійка багатоагентна робота"],
  ],
  runtimeLink: "Інтерфейси рантайму та нотатки про стабільність →",

  installBandHeading: "Почніть з однієї команди.",
  binaries: "Бінарні файли",
  chinaMirrors: "дзеркала в Китаї",
  installGuideLink: "Читати посібник зі встановлення →",

  sealCommunity: "众",
  communityHeading: "Розробляємо відкрито",
  communityBody:
    "Ліцензія MIT; проєкт формують учасники, що працюють над рантаймами, провайдерами, платформами, документацією та тестами.",
  communityLinksAria: "Посилання спільноти",
  contribute: "Участь",
};
