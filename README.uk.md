<!-- source: README.md sha256:e421b94e9f7e -->
# Codewhale

Codewhale — це агент програмування з відкритим кодом для вашого термінала, створений на Rust і вдосконалюваний публічно разом із людьми, які ним користуються.

![Codewhale працює в терміналі](assets/screenshot.webp)

[English](README.md) · [简体中文](README.zh-CN.md) · [日本語](README.ja-JP.md) · [Tiếng Việt](README.vi.md) · [Bahasa Indonesia](README.id.md) · [한국어](README.ko-KR.md) · [Español](README.es-419.md) · [Português](README.pt-BR.md) · [Русский](README.ru.md) · [Français](README.fr.md) · [Deutsch](README.de.md) · [繁體中文](README.zh-TW.md) · [हिन्दी](README.hi.md) · [Türkçe](README.tr.md) · [Italiano](README.it.md) · [Polski](README.pl.md) · [العربية](README.ar.md) · [Català](README.ca.md)

[![CI](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml/badge.svg)](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/codewhale-cli?label=crates.io)](https://crates.io/crates/codewhale-cli)
[![npm](https://img.shields.io/npm/v/codewhale?label=npm)](https://www.npmjs.com/package/codewhale)
[![Discord](https://img.shields.io/badge/Discord-join-5865F2?logo=discord&logoColor=white)](https://discord.gg/37gfS3ksug)

## Встановлення

```bash
npm install -g codewhale
codewhale
```

Під час першого запуску Codewhale допоможе під’єднати провайдера або залишитися в автономному режимі. Він також підтримує Cargo, Docker, Nix, Scoop, готові архіви, Android/Termux і дзеркало CNB. Див. [посібник зі встановлення](docs/INSTALL.md).

Для автодоповнення за Tab достатньо однієї команди для кожної оболонки — `codewhale completion bash|zsh|fish|powershell|elvish`. Див. [автодоповнення оболонки](docs/INSTALL.md#8-shell-completions).

## Використання

Спілкуйтеся з Codewhale так само, як із колегою по команді:

```text
Fix the failing tests and explain what changed.
```

Також можна запустити завдання, не відкриваючи TUI:

```bash
codewhale exec "fix the failing tests and explain what changed"
```

Codewhale може читати ваш репозиторій, редагувати файли, виконувати команди, перевіряти результати й продовжувати роботу над метою. Ви самі вирішуєте, який доступ йому надати.

## Чому Codewhale

- **Використовуйте потрібну вам модель.** Під’єднуйте хостингових провайдерів або локальні моделі через Ollama, vLLM чи SGLang. Змінюйте провайдера й модель за допомогою `/model`.
- **Зберігайте контроль.** Режим Plan доступний лише для читання. Ask, Auto-Review і Full Access наочно показують поведінку погоджень. `/undo` скасовує останній хід, а `/restore` повертає робочий простір до попереднього знімка.
- **Упорядковуйте тривалу роботу.** Зберігайте сеанси, установлюйте постійну `/goal`, перевіряйте робочі процеси перед запуском і координуйте агентів так, щоб їхні внутрішні інструкції не потрапляли до вашої розмови.
- **Розширюйте вже наявного агента.** Під’єднуйте сервери MCP і навички, налаштовуйте хуки та зберігайте ролі агентів як зрозумілі файли у своєму проєкті або особистих налаштуваннях.

Виконайте `/help` у TUI, щоб переглянути команди й клавіатурні скорочення.

## Безпека

Codewhale працює на вашому комп’ютері з доступом, який ви йому надали. Режими погодження та правила репозиторію обмежують дії агента; додаткова пісочниця ОС створює надійнішу межу виконання там, де вона підтримується. Невідома ціна моделі залишається позначеною як невідома, а не подається як безкоштовна.

Точний порядок застосування політик описано в розділі [порядок авторизації](docs/AUTHORIZATION_ORDER.md), а локальні налаштування — у розділі [конфігурація](docs/CONFIGURATION.md).

## Документація

- [Провайдери та локальні моделі](docs/PROVIDERS.md)
- [Команди агентів](docs/FLEET.md)
- [MCP](docs/MCP.md), [хуки](docs/HOOKS.md) і [конфігурація](docs/CONFIGURATION.md)
- [Локальний вебклієнт](docs/WEB.md)
- [Уся документація](docs)

## Долучайтеся до спільноти

Codewhale стає кращим, коли люди користуються ним, повідомляють про незручності й допомагають їх виправляти. Якщо потрібного провайдера немає, робочий процес незручний або інтерфейс термінала заважає роботі, [створіть issue](https://github.com/Hmbown/CodeWhale/issues). Якщо ви знаєте, як це поліпшити, [відкрийте pull request](CONTRIBUTING.md). Ми раді першим внескам, а авторство прийнятої роботи зберігається за учасниками.

Долучайтеся до [Discord](https://discord.gg/37gfS3ksug) або додайте Hunter у WeChat (`hunterbown`) і попросіть приєднати вас до групи Whale Brothers.

## Історія проєкту

Codewhale починався як `deepseek-tui` і досі зберігає сумісність із його конфігурацією та сеансами. Тепер він нейтральний щодо провайдерів, підтримується незалежно й не пов’язаний із жодним постачальником моделей.

Дякуємо всім учасникам і спільнотам відкритого коду, які допомогли проєкту зрости. Див. [список учасників](docs/CONTRIBUTORS.md).

## Ліцензія

[MIT](LICENSE). Частини, адаптовані з інших проєктів із відкритим кодом, зазначено в [повідомленнях про сторонні компоненти](docs/THIRD_PARTY_NOTICES.md).
