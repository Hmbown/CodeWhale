<!-- source: README.md sha256:e421b94e9f7e -->
# Codewhale

Codewhale ist ein in Rust entwickelter Open-Source-Coding-Agent für dein Terminal, der gemeinsam mit seinen Nutzerinnen und Nutzern öffentlich weiterentwickelt wird.

![Codewhale in einem Terminal](assets/screenshot.webp)

[English](README.md) · [简体中文](README.zh-CN.md) · [日本語](README.ja-JP.md) · [Tiếng Việt](README.vi.md) · [Bahasa Indonesia](README.id.md) · [한국어](README.ko-KR.md) · [Español](README.es-419.md) · [Português](README.pt-BR.md) · [Русский](README.ru.md) · [Українська](README.uk.md) · [Français](README.fr.md) · [繁體中文](README.zh-TW.md) · [हिन्दी](README.hi.md) · [Türkçe](README.tr.md) · [Italiano](README.it.md) · [Polski](README.pl.md) · [العربية](README.ar.md) · [Català](README.ca.md)

[![CI](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml/badge.svg)](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/codewhale-cli?label=crates.io)](https://crates.io/crates/codewhale-cli)
[![npm](https://img.shields.io/npm/v/codewhale?label=npm)](https://www.npmjs.com/package/codewhale)
[![Discord](https://img.shields.io/badge/Discord-join-5865F2?logo=discord&logoColor=white)](https://discord.gg/37gfS3ksug)

## Installation

```bash
npm install -g codewhale
codewhale
```

Beim ersten Start hilft dir Codewhale, einen Anbieter zu verbinden oder offline zu bleiben. Außerdem werden Cargo, Docker, Nix, Scoop, vorgefertigte Archive, Android/Termux und ein CNB-Spiegel unterstützt. Siehe [Installationsanleitung](docs/INSTALL.md).

Die Tab-Vervollständigung lässt sich für jede Shell mit einem einzigen Befehl aktivieren — `codewhale completion bash|zsh|fish|powershell|elvish`. Siehe [Shell-Vervollständigung](docs/INSTALL.md#8-shell-completions).

## Verwendung

Sprich mit Codewhale so, wie du mit einem Teammitglied sprechen würdest:

```text
Fix the failing tests and explain what changed.
```

Du kannst eine Aufgabe auch ausführen, ohne die TUI zu öffnen:

```bash
codewhale exec "fix the failing tests and explain what changed"
```

Codewhale kann dein Repository lesen, Dateien bearbeiten, Befehle ausführen, Ergebnisse prüfen und auf ein Ziel hinarbeiten. Du entscheidest, wie viel Zugriff der Agent erhält.

## Warum Codewhale

- **Nutze das gewünschte Modell.** Verbinde gehostete Anbieter oder lokale Modelle über Ollama, vLLM oder SGLang. Mit `/model` wechselst du Anbieter und Modell.
- **Behalte die Kontrolle.** Plan ist schreibgeschützt. Ask, Auto-Review und Full Access machen das Genehmigungsverhalten sichtbar. `/undo` macht die letzte Interaktion rückgängig und `/restore` setzt den Arbeitsbereich auf einen früheren Snapshot zurück.
- **Halte lange Arbeiten übersichtlich.** Speichere Sitzungen, setze ein dauerhaftes `/goal`, prüfe Workflows vor der Ausführung und koordiniere Agenten, ohne dass ihre internen Anweisungen in deinem Gesprächsverlauf erscheinen.
- **Erweitere deinen vorhandenen Agenten.** Verbinde MCP-Server und Skills, konfiguriere Hooks und verwalte Agentenrollen als lesbare Dateien in deinem Projekt oder in deinen persönlichen Einstellungen.

Führe `/help` in der TUI aus, um Befehle und Tastenkürzel anzuzeigen.

## Sicherheit

Codewhale läuft auf deinem Rechner mit den von dir gewährten Zugriffsrechten. Genehmigungsmodi und Repository-Regeln begrenzen, was der Agent tun darf; optionales OS-Sandboxing schafft auf unterstützten Systemen eine stärkere Ausführungsgrenze. Unbekannte Modellpreise bleiben als unbekannt gekennzeichnet, statt als kostenlos gemeldet zu werden.

Lies die [Autorisierungsreihenfolge](docs/AUTHORIZATION_ORDER.md) für die genaue Richtlinienhierarchie und die [Konfiguration](docs/CONFIGURATION.md) für lokale Einstellungen.

## Dokumentation

- [Anbieter und lokale Modelle](docs/PROVIDERS.md)
- [Agententeams](docs/FLEET.md)
- [MCP](docs/MCP.md), [Hooks](docs/HOOKS.md) und [Konfiguration](docs/CONFIGURATION.md)
- [Lokaler Webclient](docs/WEB.md)
- [Gesamte Dokumentation](docs)

## Der Community beitreten

Codewhale wird besser, wenn Menschen es nutzen, Probleme melden und bei der Behebung helfen. Wenn ein Anbieter fehlt, ein Workflow umständlich ist oder dir die Terminaloberfläche im Weg steht, [eröffne ein Issue](https://github.com/Hmbown/CodeWhale/issues). Wenn du weißt, wie es besser geht, [eröffne einen Pull Request](CONTRIBUTING.md). Erste Beiträge sind willkommen, und Mitwirkende behalten die Anerkennung für ihre übernommenen Arbeiten.

Tritt unserem [Discord](https://discord.gg/37gfS3ksug) bei oder füge Hunter auf WeChat (`hunterbown`) hinzu und bitte um Aufnahme in die Whale-Brothers-Gruppe.

## Projektgeschichte

Codewhale begann als `deepseek-tui` und bewahrt weiterhin die Kompatibilität mit dessen Konfiguration und Sitzungen. Heute ist es anbieterneutral, wird unabhängig gepflegt und ist mit keinem Modellanbieter verbunden.

Vielen Dank an alle Mitwirkenden und die Open-Source-Communitys, die das Projekt beim Wachsen unterstützt haben. Siehe [Liste der Mitwirkenden](docs/CONTRIBUTORS.md).

## Lizenz

[MIT](LICENSE). Aus anderen Open-Source-Projekten übernommene Teile sind in den [Hinweisen zu Drittanbieterkomponenten](docs/THIRD_PARTY_NOTICES.md) aufgeführt.
