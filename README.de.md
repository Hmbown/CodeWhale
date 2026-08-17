<!-- source: README.md sha256:4fc19c5f9596 -->
# Codewhale

Ein Open-Source-Programmieragent für Ihr Terminal — bringen Sie Ihr eigenes Modell mit.

Codewhale begann als native Erfahrung für DeepSeek. Daraus ist ein von der
Community getragenes Projekt geworden: ein Coding-Harness, das zu einer
wachsenden internationalen Community passt und so viele Modelle und Anbieter
wie möglich unterstützt — offene Modelle zuerst, gehostet oder lokal, keines
bevorzugt vor den anderen.

Geben Sie ihm einen Anbieter, ein Modell und eine Aufgabe. Es liest Ihren
Code, bearbeitet Dateien, führt Befehle aus und prüft die eigene Arbeit,
und hält an, wenn die Aufgabe erledigt ist oder es Sie braucht. Wechseln
Sie mitten in der Aufgabe das Modell mit `/model`. Arbeiten Sie interaktiv
in der TUI, oder starten Sie `codewhale exec` in Skripten und CI.
Geschrieben in Rust, lizenziert unter MIT, läuft es auf Ihrem Rechner.

Was es von anderen Harnessen unterscheidet: **Sie wählen das Modell für
jede Rolle, und sie müssen nicht übereinstimmen.** Eine Fleet legt
Anbieter, Modell und Reasoning-Stufe pro Rolle fest — sodass ein günstiges,
schnelles Modell ein teures Reasoning-Modell führen kann, oder ein
GLM-Builder dieselbe Aufgabe bearbeitet wie ein Kimi-Reviewer. Schreiben
Sie Ihre eigenen Rollen und Ihre eigene constitution, und der Harness
gehört Ihnen, nicht uns.

Wir suchen immer Mitwirkende und Wege, besser zu werden. Fehlt ein Modell
oder ein Anbieter, den Sie nutzen, oder bricht etwas, uns das zu sagen ist
eines der nützlichsten Dinge, die Sie tun können — siehe
[Mitwirken](#mitwirken).

[English](README.md) · [简体中文](README.zh-CN.md) · [日本語](README.ja-JP.md) · [Tiếng Việt](README.vi.md) · [Bahasa Indonesia](README.id.md) · [한국어](README.ko-KR.md) · [Español](README.es-419.md) · [Português](README.pt-BR.md) · [Русский](README.ru.md) · [Українська](README.uk.md) · [Français](README.fr.md) · [繁體中文](README.zh-TW.md) · [हिन्दी](README.hi.md) · [Türkçe](README.tr.md) · [Italiano](README.it.md) · [Polski](README.pl.md) · [العربية](README.ar.md) · [Català](README.ca.md) · [codewhale.net](https://codewhale.net/) · [Docs](docs) · [Changelog](CHANGELOG.md) · [Discord](https://discord.gg/37gfS3ksug)

[![CI](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml/badge.svg)](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/codewhale-cli?label=crates.io)](https://crates.io/crates/codewhale-cli)
[![npm](https://img.shields.io/npm/v/codewhale?label=npm)](https://www.npmjs.com/package/codewhale)
[![Discord](https://img.shields.io/badge/Discord-join%20the%20community-5865F2?logo=discord&logoColor=white)](https://discord.gg/37gfS3ksug)

![Codewhale läuft in einem Terminal](assets/screenshot.png)

## Installation

```bash
npm install -g codewhale
```

Cargo, Docker, Nix, Scoop, vorkompilierte Archive, Android/Termux und ein
CNB-Spiegel für alle, die GitHub nicht erreichen, stehen in
[docs/INSTALL.md](docs/INSTALL.md). Kommen Sie von `deepseek-tui`? Ihre
Konfiguration und Sitzungen bleiben erhalten — siehe
[docs/REBRAND.md](docs/REBRAND.md).

## Verwendung

```bash
codewhale auth set --provider deepseek   # or export ANTHROPIC_API_KEY, etc.
codewhale                                # open the TUI
codewhale exec "fix the failing test"    # headless
codewhale web                            # local browser client on 127.0.0.1
```

In der TUI: `/model` wechselt Anbieter und Modell gemeinsam, `/fleet` baut
das Team auf und führt es — eine Rolle nach der anderen, jede mit eigenem
Modell —, `/undo` macht den letzten Zug rückgängig, und `/restore <N>`
setzt den Workspace auf einen früheren Snapshot zurück (bloßes `/restore`
listet sie). `Tab` wechselt Plan / Work / Operate, wenn der Composer leer
ist — mit Text darin vervollständigt `Tab` stattdessen Slash-Befehle und
`@`-Erwähnungen. `Shift+Tab` wechselt jederzeit die
Berechtigungshaltung Ask / Auto-Review / Full Access. `!` führt einen
Shell-Befehl über den normalen Freigabepfad aus.

## Was es tut

- **Jedes Modell, jeder Anbieter — und jede Mischung.** DeepSeek, Claude,
  GPT, Kimi, GLM und mehr als 30 Anbieter, plus Ihr eigenes vLLM, SGLang
  oder Ollama ohne Schlüssel, alles über eine Runtime und einen
  Werkzeugsatz. Der Katalog verfolgt die aktuelle Palette jedes Anbieters
  — DeepSeeks V4-Pro-Backend (beschriftet `DeepSeek-V4-Pro-0813`) bleibt
  als `deepseek-v4-pro` aufrufbar, Grok 4.6 ist der direkte xAI-Standard,
  und OrcaRouter routet über `orcarouter/auto`. Eine gespeicherte Rolle
  hält `provider`, `model` und Reasoning-Stufe ausdrücklich fest, sodass
  eine Fleet in einem Lauf mehrere Anbieter umspannen kann und die Route
  einer Rolle nie davon abhängt, welcher Anbieter gerade aktiv ist.
  Kontextlimits und Preise kommen von der echten Route; ein unbekannter
  Preis erscheint als unbekannt, nicht als 0 $.
- **Ein Harness, den Sie schreiben.** Rollen sind Dateien, die Sie lesen
  und bearbeiten können — ein Modell, eine Werkzeughaltung und stehende
  Anweisungen pro Rolle — im Projekt, damit das Team sie teilt, oder
  neben Ihren anderen persönlichen Einstellungen, damit sie Ihnen von
  Repo zu Repo folgen. Eine constitution hält fest, wie sich der Agent
  über alle Sitzungen verhalten soll, sodass der Harness Ihrer Praxis
  folgt, nicht unserer.
- **Nur lesen, bis Sie mehr erlauben.** Der Plan-Modus kann keine Dateien
  ändern, und Freigaben sperren riskante Befehle. Wenn eine OS-Sandbox
  einen Befehl tatsächlich umschließt, sagt Codewhale das: Seatbelt auf
  macOS, sofern verfügbar, opt-in bubblewrap auf Linux. Das
  `constitution.json` eines Repos kompiliert zu Schreibsperren, die selbst
  Full Access nicht überspringt.
- **Arbeit, die Sie fortsetzen können.** Eine Fleet schreibt jeden Schritt
  in ein nur anhängendes Ledger, sodass `fleet resume` dort weitermacht,
  wo Sie aufgehört haben.

## Integrationen

- **DeepSeek Harness (dsh) — über Codewhale verbunden.**
  `codewhale integrations dsh connect` koppelt eine vorhandene
  `@deepseek-ai/dsh`-Installation an Ihre Codewhale-Anbieterroute,
  Berechtigungen und Ihren Workspace, und `integrations dsh
  install-bundle` fügt das optionale DSH-Plugin-Bundle hinzu, damit
  `dsh --profile codewhale` diese Identität selbst trägt. Codewhale
  besitzt Berechtigungen und Lebenszyklus-Autorität; dsh behält eigene
  Sitzungen, Profile und Anmeldedaten unangetastet. Siehe
  [docs/INTEGRATIONS_DSH.md](docs/INTEGRATIONS_DSH.md).
- **VS Code.** Das offizielle Extension-Gerüst (`extensions/vscode`) öffnet
  Codewhale in einem integrierten Terminal und stellt eine schreibgeschützte
  Agent View über die lokale Runtime bereit. Das ist eine lokale
  Entwicklungsvorschau, noch keine Marketplace-Veröffentlichung.

## Mehr erfahren

- [docs/PROVIDERS.md](docs/PROVIDERS.md) — jede Anbieterroute: gehostet,
  Gateway und lokal
- [docs/FLEET.md](docs/FLEET.md) — Fleets, das Ledger und Fortsetzen
- [docs/WORKFLOW_EXPERIMENTAL_SEARCH.md](docs/WORKFLOW_EXPERIMENTAL_SEARCH.md) — eingefrorene, anbieterneutrale experimentelle Suche in Workflow
- [docs/CONFIGURATION.md](docs/CONFIGURATION.md) — `config.toml`, Hooks
  und die constitution
- [docs/AUTHORIZATION_ORDER.md](docs/AUTHORIZATION_ORDER.md) — wie
  Modi, Hooks, Berechtigungsregeln, Sicherheitsuntergrenzen, Repo-Recht,
  Freigaben und Sandboxing zusammenwirken
- [docs/HOOKS.md](docs/HOOKS.md) — die elf TUI-Lebenszyklus-Hook-Ereignisse,
  ihre Payloads und welche drei davon einen Zug steuern können
  (`codewhale exec` und die CLI-Unterbefehle feuern keine Hooks)
- [docs/WEB.md](docs/WEB.md) — der nur-loopback Browser-Client und seine
  einmalige Authentifizierungsgrenze

Alles andere — Modi, Tastenkürzel, Sandbox-Details, MCP, die Runtime-API
und die Architektur — lebt in [docs](docs) und auf
[codewhale.net](https://codewhale.net/).

## Mitwirken

Issues, PRs, Reproduktionsschritte, Logs und Feature-Wünsche sind echte
Projektarbeit, und erste Beiträge sind willkommen. Wenn ein PR so nicht
gemergt werden kann, ernten die Maintainer, was funktioniert, und der
Autor bleibt genannt — im Commit, im Changelog und in
[docs/CONTRIBUTORS.md](docs/CONTRIBUTORS.md).

- [Offene Issues](https://github.com/Hmbown/CodeWhale/issues) — gute
  erste Beiträge liegen hier
- [CONTRIBUTING.md](CONTRIBUTING.md) — Dev-Setup und PR-Ablauf
- [docs/CONTRIBUTORS.md](docs/CONTRIBUTORS.md) — alle, die das Projekt
  mitgeformt haben
- [Spendieren Sie mir einen Kaffee](https://www.buymeacoffee.com/hmbown)

Danke an [DeepSeek](https://github.com/deepseek-ai) für die Modelle und
die Unterstützung, mit denen das Projekt begann, an
[DataWhale](https://github.com/datawhalechina) 🐋 für die Aufnahme in die
Whale-Brother-Familie, und an
[OpenWarp](https://github.com/zerx-lab/warp) und
[Open Design](https://github.com/nexu-io/open-design) für die
Zusammenarbeit an der Terminal-Agent-Erfahrung.

## Lizenz

[MIT](LICENSE). Ein unabhängiges Community-Projekt, nicht verbunden mit
einem Modellanbieter.

![Codewhale fächert drei schreibgeschützte Scout-Subagenten in einem Terminal auf](assets/fanout.gif)
