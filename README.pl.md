<!-- source: README.md sha256:4fc19c5f9596 -->
# Codewhale

Otwartoźródłowy agent programistyczny do Twojego terminala — model przynosisz sam.

Codewhale zaczął jako natywne doświadczenie dla DeepSeek. Od tego czasu urósł
do projektu prowadzonego przez społeczność: jednego harnessu programistycznego,
który pasuje do rosnącej międzynarodowej społeczności i obsługuje tyle modeli
i dostawców, ile się da — najpierw modele otwarte, hostowane albo lokalne,
żaden nie jest uprzywilejowany wobec reszty.

Daj mu dostawcę, model i zadanie. Czyta Twój kod, edytuje pliki, uruchamia
polecenia i sprawdza własną pracę, a potem się zatrzymuje, gdy zadanie jest
skończone albo potrzebuje Ciebie. W trakcie zadania zmień model przez
`/model`. Pracuj interaktywnie w TUI albo uruchamiaj `codewhale exec` w
skryptach i CI. Napisany w Rust, na licencji MIT, działa na Twojej maszynie.

To, czym nie przypomina innych harnessów: **to Ty wybierasz model dla każdej
roli i nie muszą się zgadzać.** Fleet przypina dostawcę, model i poziom
rozumowania do roli — więc tani, szybki model może kierować drogim modelem
rozumującym, a builder GLM może robić to samo zadanie co reviewer Kimi. Napisz
własne role i własną constitution, a harness będzie Twój, nie nasz.

Zawsze szukamy osób, które chcą się przyczynić, i sposobów na ulepszenia. Jeśli
brakuje modelu albo dostawcy, z którego korzystasz, albo coś się psuje,
powiedzenie nam o tym jest jedną z najpożyteczniejszych rzeczy, które możesz
zrobić — zobacz [Współtworzenie](#współtworzenie).

[English](README.md) · [简体中文](README.zh-CN.md) · [日本語](README.ja-JP.md) · [Tiếng Việt](README.vi.md) · [Bahasa Indonesia](README.id.md) · [한국어](README.ko-KR.md) · [Español](README.es-419.md) · [Português](README.pt-BR.md) · [Русский](README.ru.md) · [Українська](README.uk.md) · [Français](README.fr.md) · [Deutsch](README.de.md) · [繁體中文](README.zh-TW.md) · [हिन्दी](README.hi.md) · [Türkçe](README.tr.md) · [Italiano](README.it.md) · [العربية](README.ar.md) · [Català](README.ca.md) · [codewhale.net](https://codewhale.net/) · [Docs](docs) · [Changelog](CHANGELOG.md) · [Discord](https://discord.gg/37gfS3ksug)

[![CI](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml/badge.svg)](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/codewhale-cli?label=crates.io)](https://crates.io/crates/codewhale-cli)
[![npm](https://img.shields.io/npm/v/codewhale?label=npm)](https://www.npmjs.com/package/codewhale)
[![Discord](https://img.shields.io/badge/Discord-join%20the%20community-5865F2?logo=discord&logoColor=white)](https://discord.gg/37gfS3ksug)

![Codewhale działający w terminalu](assets/screenshot.png)

## Instalacja

```bash
npm install -g codewhale
```

Cargo, Docker, Nix, Scoop, gotowe archiwa, Android/Termux oraz lustro CNB
dla tych, którzy nie dochodzą do GitHuba, są w
[docs/INSTALL.md](docs/INSTALL.md). Przychodzisz z `deepseek-tui`? Konfiguracja
i sesje przechodzą — zobacz [docs/REBRAND.md](docs/REBRAND.md).

## Użycie

```bash
codewhale auth set --provider deepseek   # or export ANTHROPIC_API_KEY, etc.
codewhale                                # open the TUI
codewhale exec "fix the failing test"    # headless
codewhale web                            # local browser client on 127.0.0.1
```

W TUI: `/model` przełącza dostawcę i model razem, `/fleet` buduje i uruchamia
zespół — jedna rola na raz, każda z własnym modelem —, `/undo` cofa ostatnią
turę, a `/restore <N>` cofa przestrzeń roboczą do wcześniejszego zrzutu
(gołe `/restore` je wypisuje). `Tab` przełącza Plan / Work / Operate, gdy
kompozytor jest pusty — z tekstem `Tab` uzupełnia polecenia slash i wzmianki
`@`. `Shift+Tab` w każdej chwili przełącza postawę uprawnień Ask /
Auto-Review / Full Access. `!` uruchamia polecenie powłoki zwykłą ścieżką
zatwierdzeń.

## Co robi

- **Dowolny model, dowolny dostawca — i dowolna mieszanka.** DeepSeek, Claude,
  GPT, Kimi, GLM i ponad 30 dostawców, plus własne vLLM, SGLang albo Ollama
  bez klucza, wszystko przez jedno środowisko uruchomieniowe i jeden zestaw
  narzędzi. Katalog śledzi żywą ofertę każdego dostawcy — backend V4 Pro
  DeepSeek (etykieta `DeepSeek-V4-Pro-0813`) nadal wywołuje się jako
  `deepseek-v4-pro`, Grok 4.6 jest bezpośrednim domyślnym modelem xAI, a
  OrcaRouter kieruje przez `orcarouter/auto`. Zapisana rola jawnie notuje
  `provider`, `model` i poziom rozumowania, więc fleet może w jednym
  uruchomieniu objąć wielu dostawców, a trasa roli nigdy nie zależy od tego,
  który dostawca akurat jest aktywny. Limity kontekstu i ceny pochodzą z
  prawdziwej trasy; nieznana cena pokazuje się jako nieznana, nie jako 0 $.
- **Harness, który sam piszesz.** Role to pliki, które możesz czytać i
  edytować — model, postawa narzędzi i stałe instrukcje na rolę — trzymane
  w projekcie, żeby zespół je dzielił, albo obok innych ustawień osobistych,
  żeby szły za Tobą między repozytoriami. Constitution zapisuje, jak agent
  ma się zachowywać w każdej sesji, żeby harness pasował do Twojej praktyki,
  nie do naszej.
- **Tylko odczyt, dopóki nie pozwolisz na więcej.** Tryb Plan nie może
  zmieniać plików, a zatwierdzenia pilnują ryzykownych poleceń. Gdy sandbox
  systemu operacyjnego naprawdę otacza polecenie, Codewhale o tym mówi:
  Seatbelt na macOS, gdy jest dostępny, opcjonalny bubblewrap na Linuksie.
  `constitution.json` repozytorium kompiluje się do blokad zapisu, których
  nawet Full Access nie przeskoczy.
- **Praca, którą możesz wznowić.** Fleet zapisuje każdy krok w rejestrze
  tylko do dopisywania, więc `fleet resume` podejmuje tam, gdzie skończyłeś.

## Integracje

- **DeepSeek Harness (dsh) — podłączony przez Codewhale.**
  `codewhale integrations dsh connect` wiąże istniejącą instalację
  `@deepseek-ai/dsh` z Twoją trasą dostawcy Codewhale, uprawnieniami i
  przestrzenią roboczą, a `integrations dsh install-bundle` dodaje opcjonalny
  pakiet wtyczek DSH, żeby `dsh --profile codewhale` nosił tę tożsamość sam.
  Codewhale ma uprawnienia i władzę nad cyklem życia; dsh zostawia własne
  sesje, profile i poświadczenia nietknięte. Zobacz
  [docs/INTEGRATIONS_DSH.md](docs/INTEGRATIONS_DSH.md).
- **VS Code.** Oficjalny szkielet rozszerzenia (`extensions/vscode`) otwiera
  Codewhale w zintegrowanym terminalu i udostępnia Agent View tylko do odczytu
  nad lokalnym środowiskiem. To lokalny podgląd deweloperski, jeszcze nie
  wydanie na marketplace.

## Dowiedz się więcej

- [docs/PROVIDERS.md](docs/PROVIDERS.md) — każda trasa dostawcy: hostowana,
  bramka i lokalna
- [docs/FLEET.md](docs/FLEET.md) — fleety, rejestr i wznawianie
- [docs/WORKFLOW_EXPERIMENTAL_SEARCH.md](docs/WORKFLOW_EXPERIMENTAL_SEARCH.md) — zamrożone, niezależne od dostawcy eksperymentalne wyszukiwanie w Workflow
- [docs/CONFIGURATION.md](docs/CONFIGURATION.md) — `config.toml`, hooki i
  constitution
- [docs/AUTHORIZATION_ORDER.md](docs/AUTHORIZATION_ORDER.md) — jak składają
  się tryby, hooki, reguły uprawnień, podłogi bezpieczeństwa, prawo repo,
  zatwierdzenia i sandbox
- [docs/HOOKS.md](docs/HOOKS.md) — jedenaście zdarzeń hook cyklu życia TUI,
  ich ładunki i które trzy mogą pokierować turą (`codewhale exec` i
  podpolecenia CLI nie odpalają hooków)
- [docs/WEB.md](docs/WEB.md) — przeglądarkowy klient tylko na loopback i jego
  jednorazowa granica uwierzytelniania

Wszystko inne — tryby, skróty, szczegóły sandboxa, MCP, API środowiska
uruchomieniowego i architektura — żyje w [docs](docs) i na
[codewhale.net](https://codewhale.net/).

## Współtworzenie

Zgłoszenia, PR-y, kroki odtworzenia, logi i prośby o funkcje to prawdziwa
praca projektowa, a pierwsze wkłady są mile widziane. Gdy PR nie da się
włączyć w obecnym kształcie, opiekunowie zbierają to, co działa, a autor
zostaje wymieniony — w commicie, w changelogu i w
[docs/CONTRIBUTORS.md](docs/CONTRIBUTORS.md).

- [Otwarte zgłoszenia](https://github.com/Hmbown/CodeWhale/issues) — dobre
  pierwsze wkłady są tutaj
- [CONTRIBUTING.md](CONTRIBUTING.md) — środowisko deweloperskie i przepływ PR
- [docs/CONTRIBUTORS.md](docs/CONTRIBUTORS.md) — wszyscy, którzy to ukształtowali
- [Postaw mi kawę](https://www.buymeacoffee.com/hmbown)

Dziękujemy [DeepSeek](https://github.com/deepseek-ai) za modele i wsparcie,
które rozpoczęły projekt, [DataWhale](https://github.com/datawhalechina) 🐋
za przyjęcie nas do rodziny Whale Brother oraz
[OpenWarp](https://github.com/zerx-lab/warp) i
[Open Design](https://github.com/nexu-io/open-design) za współpracę przy
doświadczeniu agenta w terminalu.

## Licencja

[MIT](LICENSE). Niezależny projekt społecznościowy, niezwiązany z żadnym
dostawcą modeli.

![Codewhale rozsyła trzech podrzędnych agentów scout tylko do odczytu w terminalu](assets/fanout.gif)
