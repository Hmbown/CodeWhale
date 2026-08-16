<!-- source: README.md sha256:1569156eb887 -->
# Codewhale

Открытый агент для программирования в вашем терминале — модель приносите с собой.

Codewhale начинался как нативный клиент для DeepSeek. С тех пор он вырос в проект,
которым руководит сообщество: единый каркас для программирования, подходящий
растущему международному сообществу и поддерживающий как можно больше моделей и
провайдеров — открытые модели в первую очередь, облачные или локальные, ни один не
имеет привилегий перед остальными.

Дайте ему провайдера, модель и задачу. Он читает ваш код, правит файлы, запускает
команды и проверяет собственную работу, а затем останавливается, когда задача
выполнена или ему нужны вы. Переключайте модели прямо посреди задачи командой
`/model`. Работайте интерактивно в TUI или запускайте `codewhale exec` в скриптах
и CI. Он написан на Rust, распространяется по лицензии MIT и работает на вашей
машине.

Чем это не похоже на другие harness: **вы сами выбираете модель для каждой
роли, и они не обязаны совпадать.** Fleet закрепляет провайдера, модель и
уровень рассуждений отдельно для каждой роли — поэтому дешёвая и быстрая модель
может руководить дорогой рассуждающей, а builder на GLM может работать над той
же задачей, что и reviewer на Kimi. Опишите свои роли и свою constitution — и
harness станет вашим, а не нашим.

Мы всегда ищем участников и способы стать лучше. Если модели или провайдера,
которым вы пользуетесь, не хватает, или что-то сломалось, сообщить нам об этом —
одно из самых полезных действий с вашей стороны: см.
[Участие в проекте](#участие-в-проекте).

[English](README.md) · [简体中文](README.zh-CN.md) · [日本語](README.ja-JP.md) · [Tiếng Việt](README.vi.md) · [Bahasa Indonesia](README.id.md) · [한국어](README.ko-KR.md) · [Español](README.es-419.md) · [Português](README.pt-BR.md) · [Українська](README.uk.md) · [codewhale.net](https://codewhale.net/) · [Docs](docs) · [Changelog](CHANGELOG.md) · [Discord](https://discord.gg/37gfS3ksug)

[![CI](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml/badge.svg)](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/codewhale-cli?label=crates.io)](https://crates.io/crates/codewhale-cli)
[![npm](https://img.shields.io/npm/v/codewhale?label=npm)](https://www.npmjs.com/package/codewhale)
[![Discord](https://img.shields.io/badge/Discord-join%20the%20community-5865F2?logo=discord&logoColor=white)](https://discord.gg/37gfS3ksug)

![Codewhale, запущенный в терминале](assets/screenshot.png)

## Установка

```bash
npm install -g codewhale
```

Cargo, Docker, Nix, Scoop, готовые архивы, Android/Termux и зеркало CNB для тех,
кто не может получить доступ к GitHub, описаны в
[docs/INSTALL.md](docs/INSTALL.md). Переходите с `deepseek-tui`? Ваши настройки и
сессии переносятся автоматически — см. [docs/REBRAND.md](docs/REBRAND.md).

## Использование

```bash
codewhale auth set --provider deepseek   # or export ANTHROPIC_API_KEY, etc.
codewhale                                # open the TUI
codewhale exec "fix the failing test"    # headless
codewhale web                            # local browser client on 127.0.0.1
```


В TUI: `/model` переключает провайдера и модель одновременно, `/fleet` запускает
команду воркеров, `/undo` отменяет последний ход, а `/restore <N>` откатывает
рабочую копию к более раннему снимку (`/restore` без аргумента только выводит их
список). Когда поле ввода пустое, `Tab` циклически переключает режимы Plan /
Work / Operate; если в поле есть текст, `Tab` дополняет слэш-команды и упоминания
`@`. `Shift+Tab` переключает уровни прав Ask / Auto-Review / Full Access в любой
момент. `!` запускает команду оболочки через обычный путь подтверждения.

## Что он умеет

- **Любая модель, любой провайдер.** DeepSeek, Claude, GPT, Kimi, GLM и более 30
  провайдеров, плюс ваши собственные vLLM, SGLang или Ollama без ключа — всё
  через единый рантайм и единый набор инструментов. Каталог отслеживает актуальный состав каждого провайдера: бэкенд DeepSeek V4 Pro (с меткой `DeepSeek-V4-Pro-0813`) по-прежнему вызывается как `deepseek-v4-pro`, Grok 4.6 — модель по умолчанию для прямого маршрута xAI, а OrcaRouter маршрутизирует через `orcarouter/auto`. Лимиты контекста и цены
  берутся из реального маршрута, а неизвестная цена отображается как неизвестная,
  а не как $0.
- **Harness, который пишете вы.** Роли — это файлы, которые можно прочитать и
  изменить: для каждой роли своя модель, своя позиция по инструментам и
  постоянные инструкции. Держите их в проекте, чтобы ими пользовалась команда,
  или рядом с личными настройками, чтобы они следовали за вами между
  репозиториями. Constitution фиксирует, как вы хотите, чтобы агент вёл себя в
  каждой сессии, — так harness подстраивается под вашу практику, а не под нашу.
- **Только чтение, пока вы не разрешите больше.** Режим Plan не может изменять
  файлы, а рискованные команды требуют подтверждения. Когда команду действительно
  оборачивает песочница ОС, Codewhale сообщает об этом: Seatbelt на macOS, где он
  доступен, и опциональный bubblewrap на Linux. Файл `constitution.json` в
  репозитории компилируется в блокировки записи, которые не может обойти даже
  Full Access.
- **Работа, которую можно продолжить.** Флит записывает каждый шаг в журнал,
  доступный только на добавление, поэтому `fleet resume` продолжает с того места,
  где вы остановились.

## Интеграции

- **DeepSeek Harness (dsh) — подключается через Codewhale.**
  `codewhale integrations dsh connect` связывает существующую установку
  `@deepseek-ai/dsh` с вашим маршрутом провайдера, правами и рабочей
  областью Codewhale; `integrations dsh install-bundle` добавляет
  опциональный бандл-плагин DSH, чтобы `dsh --profile codewhale` нёс эту
  идентичность самостоятельно. Права и жизненный цикл остаются за
  Codewhale; сессии, профили и учётные данные dsh не затрагиваются. См.
  [docs/INTEGRATIONS_DSH.md](docs/INTEGRATIONS_DSH.md).
- **VS Code.** Официальный каркас расширения (`extensions/vscode`) открывает
  Codewhale во встроенном терминале и даёт read-only Agent View поверх
  локального рантайма. Это превью для локальной разработки, а не релиз в
  маркетплейсе.

## Узнать больше

- [docs/PROVIDERS.md](docs/PROVIDERS.md) — все маршруты провайдеров: облачные,
  шлюзы и локальные
- [docs/FLEET.md](docs/FLEET.md) — флиты, журнал и возобновление работы
- [docs/WORKFLOW_EXPERIMENTAL_SEARCH.md](docs/WORKFLOW_EXPERIMENTAL_SEARCH.md) —
  замороженный, нейтральный к провайдерам экспериментальный поиск внутри
  Workflow
- [docs/CONFIGURATION.md](docs/CONFIGURATION.md) — `config.toml`, хуки и
  constitution
- [docs/AUTHORIZATION_ORDER.md](docs/AUTHORIZATION_ORDER.md) — как сочетаются
  режимы, хуки, правила разрешений, минимальные требования безопасности, правила
  репозитория, подтверждения и песочница
- [docs/HOOKS.md](docs/HOOKS.md) — одиннадцать событий хуков жизненного цикла
  TUI, их полезная нагрузка и три из них, способные направлять ход (`codewhale
  exec` и подкоманды CLI хуки не запускают)
- [docs/WEB.md](docs/WEB.md) — браузерный клиент, работающий только на loopback,
  и его одноразовая граница аутентификации

Всё остальное — режимы, сочетания клавиш, подробности о песочнице, MCP, API
рантайма и архитектура — находится в [docs](docs) и на
[codewhale.net](https://codewhale.net/).

## Участие в проекте

Задачи, PR, шаги воспроизведения, логи и запросы функций — всё это настоящая
работа над проектом, и первые вклады приветствуются. Когда PR нельзя влить как
есть, мейнтейнеры забирают работающие части, сохраняя авторство — в коммите, в
списке изменений и в [docs/CONTRIBUTORS.md](docs/CONTRIBUTORS.md).

- [Открытые задачи](https://github.com/Hmbown/CodeWhale/issues) — здесь живут
  хорошие задачи для первого вклада
- [CONTRIBUTING.md](CONTRIBUTING.md) — настройка среды разработки и процесс PR
- [docs/CONTRIBUTORS.md](docs/CONTRIBUTORS.md) — все, кто сформировал этот проект
- [Buy me a coffee](https://www.buymeacoffee.com/hmbown)

Благодарим [DeepSeek](https://github.com/deepseek-ai) за модели и поддержку, с
которых начался проект, [DataWhale](https://github.com/datawhalechina) 🐋 за
теплый приём в семью «Whale Brother», а также
[OpenWarp](https://github.com/zerx-lab/warp) и
[Open Design](https://github.com/nexu-io/open-design) за сотрудничество в
создании терминального агента.

## Лицензия

[MIT](LICENSE). Независимый проект сообщества, не аффилированный ни с одним
провайдером моделей.

![Codewhale запускает три read-only scout-субагента параллельно в терминале](assets/fanout.gif)