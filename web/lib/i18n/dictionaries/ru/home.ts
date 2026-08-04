import type { HomeDict } from "../types";

/**
 * Russian home dictionary. Key parity with `en/home.ts` is enforced by
 * `npm run check:locales` and `dictionaries.test.ts`.
 *
 * Fixed product vocabulary stays Latin and matches the TUI ru locale pack:
 * Plan / Act / Operate, Ask / Auto-Review / Full Access, Codewhale, Fleet.
 * "receipt" is rendered "квитанция", as in `crates/tui/locales/ru.json`.
 * The `seal*` values are the paper's marks, shared across locales.
 */
export const home: HomeDict = {
  metaTitle: "Codewhale — Мы ныряем в глубину, чтобы не пришлось вам.",
  metaDescription:
    "Codewhale ныряет в глубину, чтобы не пришлось вам, — терминальный агент, который даёт обычным людям силу больших языковых моделей, чтобы создавать вещи. Работает на вашей машине. Rust, лицензия MIT.",

  kicker: "Открытый код · Любая модель · В вашем терминале",
  heroTitleA: "Мы ныряем в глубину,",
  heroTitleB: "чтобы не пришлось вам.",
  heroIntro:
    "{brand} даёт обычным людям силу больших языковых моделей, чтобы создавать вещи. В вашем терминале он читает репозиторий, правит файлы, запускает проверки и оставляет квитанцию — не требуя, чтобы вы уже говорили на языке кода. Он работает на вашей машине; модель здесь — сменный компонент, а не сам продукт.",
  install: "Установка",
  docs: "Документация",
  copy: "Копировать",
  copied: "Скопировано ✓",

  installEyebrow: "установка одной строкой",
  installRequirement: "нужен Node 18+ — без тулчейна Rust",
  installOtherWays: "другие способы →",

  latestRelease: "Последний релиз {tag}",
  releaseUnavailable: "Статус релиза недоступен",
  currentSource: "Текущие исходники",
  sourceCandidate: "Кандидат из исходников",
  providerRoutes: "маршрутов провайдеров — {count}",
  publishedRelease: "опубликованный релиз",
  figcaptionSourceCandidate: "кандидат из исходников",

  shotSession: "Текущий сеанс",
  screenshotAlt:
    "Текущий терминальный сеанс Codewhale: режим Operate, кит, поле ввода и нижняя панель",
  figcaption: "Текущий сеанс Codewhale · режим Operate · разрешения Ask",

  proofHeading: "Подводная терминальная оболочка. Нейтральна к моделям. Работает локально.",
  proofBody:
    "Подключите облачную, шлюзовую или локальную модель, которой уже пользуетесь. Codewhale работает на вашей машине и считает модель сменным компонентом, а не продуктом. Plan / Act / Operate и явные режимы разрешений оставляют глубокое погружение под вашим контролем.",

  sealDecides: "法",
  decidesEyebrow: "Как он принимает решения",
  decidesHeading: "Правила видны прямо в ходе рассуждений",
  decidesLede:
    "Точные фрагменты реального сеанса: приоритет правил проекта виден в рассуждении модели, а не заявлен на этой странице.",

  sealWorkflow: "行",
  workflowHeading: "От задачи к проверенному изменению.",
  workflow: [
    ["Осмотр", "Читает репозиторий, его инструкции и задачу."],
    ["Действие", "Правит файлы в рамках явных границ одобрения."],
    ["Проверка", "Выполняет проверки и изучает результат."],
    ["Отчёт", "Оставляет краткую и долговечную квитанцию."],
  ],
  receiptAria: "Пример рабочей квитанции",
  receiptInspect: "репозиторий и инструкции",
  receiptAct: "правка в рамках выбранного режима разрешений",
  receiptReport: "проверки пройдены · квитанция сохранена",

  sealStart: "起",
  startHeading: "Впервые в Codewhale? Четыре шага от начала до конца.",
  startLede:
    "Установка → первый сеанс без ключей → подключение провайдера → первый воркфлоу Fleet. Термины определены на странице словаря.",
  startGuideLink: "Читать руководство «С чего начать» →",
  startVocabularyLink: "Посмотреть словарь продукта →",

  sealBoundaries: "界",
  boundariesHeadingA: "Ваша модель.",
  boundariesHeadingB: "Ваши границы.",
  boundariesBody:
    "Вы явно выбираете модель, рабочий режим и режим разрешений. Неизвестная стоимость остаётся неизвестной, а предварительные возможности прямо помечены как предварительные.",
  hostedGatewayLocal: "Облачные, шлюзовые и локальные модели",
  planActOperateDesc: "От планирования только для чтения до автономной работы",
  askAutoReviewDesc: "Выберите режим разрешений под задачу",
  tuiExecWebDesc: "Интерактивные и неинтерактивные интерфейсы рантайма",

  sealSurfaces: "面",
  surfacesHeading: "Используйте рантайм там, где идёт работа.",
  surfaces: [
    ["TUI", "Интерактивная работа в терминале"],
    ["codewhale exec", "Скрипты и CI"],
    ["Веб-клиент", "Клиент в браузере, только через loopback"],
    ["Runtime API + MCP", "Локальные интеграции"],
    ["Fleet", "Долговечная работа нескольких агентов"],
  ],
  runtimeLink: "Интерфейсы рантайма и заметки о стабильности →",

  installBandHeading: "Начните с одной команды.",
  binaries: "Бинарные сборки",
  chinaMirrors: "Зеркала в Китае",
  installGuideLink: "Читать руководство по установке →",

  sealCommunity: "众",
  communityHeading: "Разрабатывается открыто",
  communityBody:
    "Лицензия MIT; проект формируют контрибьюторы, работающие над рантаймами, провайдерами, платформами, документацией и тестами.",
  communityLinksAria: "Ссылки сообщества",
  contribute: "Участие",
};
