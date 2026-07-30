<!-- source: README.md sha256:e2678f36a915 -->
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

Мы всегда ищем участников и способы стать лучше. Если модели или провайдера,
которым вы пользуетесь, не хватает, или что-то сломалось, сообщить нам об этом —
одно из самых полезных действий с вашей стороны: см.
[Участие в проекте](#участие-в-проекте).

[English](README.md) · [简体中文](README.zh-CN.md) · [日本語](README.ja-JP.md) · [Tiếng Việt](README.vi.md) · [Bahasa Indonesia](README.id.md) · [한국어](README.ko-KR.md) · [Español](README.es-419.md) · [Português](README.pt-BR.md) · [Українська](README.uk.md) · [Українська](README.uk.md) · [codewhale.net](https://codewhale.net/) · [Docs](docs) · [Changelog](CHANGELOG.md)

[![CI](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml/badge.svg)](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/codewhale-cli?label=crates.io)](https://crates.io/crates/codewhale-cli)
[![npm](https://img.shields.io/npm/v/codewhale?label=npm)](https://www.npmjs.com/package/codewhale)

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
команду воркеров, а `/restore` отменяет ход. Когда поле ввода свободно, `Tab`
циклически переключает режимы Plan / Act / Operate, а `Shift+Tab` — уровни прав
Ask / Auto-Review / Full Access. `!` запускает команду оболочки через обычный
путь подтверждения.

## Что он умеет

- **Любая модель, любой провайдер.** DeepSeek, Claude, GPT, Kimi, GLM и более 30
  провайдеров, плюс ваши собственные vLLM, SGLang или Ollama без ключа — всё
  через единый рантайм и единый набор инструментов. Лимиты контекста и цены
  берутся из реального маршрута, а неизвестная цена отображается как неизвестная,
  а не как $0.
- **Только чтение, пока вы не разрешите больше.** Режим Plan не может изменять
  файлы, а рискованные команды требуют подтверждения. Когда команду действительно
  оборачивает песочница ОС, Codewhale сообщает об этом: Seatbelt на macOS, где он
  доступен, и опциональный bubblewrap на Linux. Файл `constitution.json` в
  репозитории компилируется в блокировки записи, которые не может обойти даже
  Full Access.
- **Работа, которую можно продолжить.** Флит записывает каждый шаг в журнал,
  доступный только на добавление, поэтому `fleet resume` продолжает с того места,
  где вы остановились.

## Узнать больше

- [docs/PROVIDERS.md](docs/PROVIDERS.md) — все маршруты провайдеров: облачные,
  шлюзы и локальные
- [docs/FLEET.md](docs/FLEET.md) — флиты, журнал и возобновление работы
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

[![Star History Chart](https://api.star-history.com/chart?repos=Hmbown/CodeWhale&type=date&legend=top-left)](https://www.star-history.com/?repos=Hmbown%2FCodeWhale&type=date)
