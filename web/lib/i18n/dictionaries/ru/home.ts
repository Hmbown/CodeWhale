import type { HomeDict } from "../types";

/**
 * Russian home dictionary — native rewrite mirroring the current English
 * direction (open-source terminal coding agent, bring your own model, runs
 * on your machine). Key parity with `en/home.ts` is enforced by
 * `npm run check:locales` and `dictionaries.test.ts`.
 *
 * Fixed product vocabulary stays Latin and matches the TUI ru locale pack:
 * Plan / Work / Operate, Ask / Auto-Review / Full Access, Codewhale, fleet.
 * "receipt" is rendered "квитанция", as in `crates/tui/locales/ru.json`.
 * The `seal*` values are the paper's marks, shared across locales.
 */
export const home: HomeDict = {
  metaTitle: "Codewhale — ныряет в глубину, чтобы вам не пришлось.",
  metaDescription:
    "Codewhale ныряет в глубину, чтобы вам не пришлось — терминальный кодинг-агент с открытым исходным кодом. Подключите свою модель. Работает на вашей машине. Rust, MIT.",

  kicker: "Открытый код · Своя модель · В вашем терминале",
  heroTitleA: "Codewhale ныряет в глубину,",
  heroTitleB: "чтобы вам не пришлось.",
  heroIntro:
    "{brand} — кодинг-агент с открытым исходным кодом для вашего терминала. Дайте ему модель и задачу — он читает ваш код, правит файлы, сам запускает проверки и останавливается, когда работа сделана или ему нужны вы. Подойдёт любая модель, а можно и смешивать: закрепите за каждой ролью свою.",
  install: "Установить",
  docs: "Документация",
  copy: "Копировать",
  copied: "Скопировано ✓",

  installEyebrow: "установка одной строкой",
  installRequirement: "нужен Node 18+ — тулчейн Rust не требуется",
  installOtherWays: "другие способы →",

  latestRelease: "Последний релиз {tag}",
  releaseUnavailable: "Статус релиза недоступен",
  currentSource: "Исходники",
  sourceCandidate: "Не выпущено",
  providerRoutes: "провайдеров — {count}",
  publishedRelease: "выпущено",
  figcaptionSourceCandidate: "не выпущено",

  shotSession: "Текущий сеанс",
  screenshotAlt:
    "Текущий терминальный сеанс Codewhale: режим Operate, кит, поле ввода и нижняя панель",
  figcaption: "Текущий сеанс Codewhale · режим Operate · режим разрешений Ask",

  proofHeading: "Подводная оболочка для терминала. Любая модель. На вашей машине.",
  proofBody:
    "Подключите модель, которой уже пользуетесь — облачную, через шлюз или локальную. Plan / Work / Operate и явные режимы разрешений держат погружение под вашим контролем.",

  sealDecides: "法",
  decidesEyebrow: "Как он принимает решения",
  decidesHeading: "Правила видны прямо в ходе рассуждений",
  decidesLede:
    "Фрагменты реальных сеансов — приоритет правил проекта виден в рассуждении модели, а не только заявлен на странице.",

  sealWorkflow: "行",
  workflowHeading: "От задачи к проверенному изменению.",
  workflow: [
    ["Осмотр", "Читает репозиторий, его инструкции и задачу."],
    ["Действие", "Правит файлы в рамках явных границ одобрения."],
    ["Проверка", "Запускает проверки и изучает результат."],
    ["Отчёт", "Оставляет краткую и долговечную квитанцию."],
  ],
  receiptAria: "Пример рабочей квитанции",
  receiptInspect: "репозиторий и инструкции",
  receiptAct: "правка в рамках выбранного режима разрешений",
  receiptReport: "проверки пройдены · квитанция сохранена",

  sealStart: "起",
  startHeading: "Впервые в Codewhale? Четыре шага от начала до конца.",
  startLede:
    "Установка → первый сеанс без ключей → подключение провайдера → первый воркфлоу fleet. Термины — на странице словаря.",
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
  tuiExecWebDesc: "Интерактивные и headless-интерфейсы рантайма",

  sealSurfaces: "面",
  surfacesHeading: "Используйте рантайм там, где идёт работа.",
  surfaces: [
    ["TUI", "Интерактивная работа в терминале"],
    ["codewhale exec", "Скрипты и CI"],
    ["Веб-клиент", "Клиент в браузере, только через loopback"],
    ["Runtime API + MCP", "Локальные интеграции"],
    ["fleet", "Длительная работа нескольких агентов"],
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
