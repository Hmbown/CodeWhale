<!-- source: README.md sha256:e421b94e9f7e -->
# Codewhale

Codewhale è un agente di programmazione open source per il terminale, sviluppato in Rust e migliorato pubblicamente insieme alle persone che lo utilizzano.

![Codewhale in esecuzione in un terminale](assets/screenshot.webp)

[English](README.md) · [简体中文](README.zh-CN.md) · [日本語](README.ja-JP.md) · [Tiếng Việt](README.vi.md) · [Bahasa Indonesia](README.id.md) · [한국어](README.ko-KR.md) · [Español](README.es-419.md) · [Português](README.pt-BR.md) · [Русский](README.ru.md) · [Українська](README.uk.md) · [Français](README.fr.md) · [Deutsch](README.de.md) · [繁體中文](README.zh-TW.md) · [हिन्दी](README.hi.md) · [Türkçe](README.tr.md) · [Polski](README.pl.md) · [العربية](README.ar.md) · [Català](README.ca.md)

[![CI](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml/badge.svg)](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/codewhale-cli?label=crates.io)](https://crates.io/crates/codewhale-cli)
[![npm](https://img.shields.io/npm/v/codewhale?label=npm)](https://www.npmjs.com/package/codewhale)
[![Discord](https://img.shields.io/badge/Discord-join-5865F2?logo=discord&logoColor=white)](https://discord.gg/37gfS3ksug)

## Installazione

```bash
npm install -g codewhale
codewhale
```

Al primo avvio, Codewhale ti aiuta a collegare un provider oppure a rimanere offline. Supporta inoltre Cargo, Docker, Nix, Scoop, archivi precompilati, Android/Termux e un mirror CNB. Consulta la [guida all’installazione](docs/INSTALL.md).

Il completamento con Tab si attiva con un solo comando per ogni shell — `codewhale completion bash|zsh|fish|powershell|elvish`. Consulta il [completamento della shell](docs/INSTALL.md#8-shell-completions).

## Utilizzo

Parla con Codewhale come parleresti con un membro del tuo team:

```text
Fix the failing tests and explain what changed.
```

Oppure esegui un’attività senza aprire la TUI:

```bash
codewhale exec "fix the failing tests and explain what changed"
```

Codewhale può leggere il tuo repository, modificare file, eseguire comandi, controllare i risultati e continuare a lavorare verso un obiettivo. Sei tu a decidere quanto accesso concedergli.

## Perché Codewhale

- **Usa il modello che preferisci.** Collega provider gestiti oppure modelli locali tramite Ollama, vLLM o SGLang. Cambia provider e modello con `/model`.
- **Mantieni il controllo.** Plan è in sola lettura. Ask, Auto-Review e Full Access rendono visibile il comportamento delle approvazioni. `/undo` annulla l’ultimo turno e `/restore` riporta l’area di lavoro a uno snapshot precedente.
- **Mantieni organizzati i lavori lunghi.** Salva le sessioni, imposta un `/goal` duraturo, rivedi i workflow prima dell’esecuzione e coordina gli agenti senza trasformare le loro istruzioni interne in parte della tua conversazione.
- **Estendi l’agente che hai già.** Collega server MCP e skill, configura gli hook e conserva i ruoli degli agenti come file leggibili nel progetto o nelle impostazioni personali.

Esegui `/help` nella TUI per vedere i comandi e le scorciatoie da tastiera.

## Sicurezza

Codewhale viene eseguito sul tuo computer con l’accesso che gli concedi. Le modalità di approvazione e le regole del repository limitano ciò che l’agente può fare; il sandboxing facoltativo del sistema operativo aggiunge un confine di esecuzione più solido dove supportato. I prezzi sconosciuti dei modelli restano indicati come sconosciuti anziché essere segnalati come gratuiti.

Leggi l’[ordine di autorizzazione](docs/AUTHORIZATION_ORDER.md) per conoscere l’esatta gerarchia delle regole e la [configurazione](docs/CONFIGURATION.md) per le impostazioni locali.

## Documentazione

- [Provider e modelli locali](docs/PROVIDERS.md)
- [Team di agenti](docs/FLEET.md)
- [MCP](docs/MCP.md), [hook](docs/HOOKS.md) e [configurazione](docs/CONFIGURATION.md)
- [Client web locale](docs/WEB.md)
- [Tutta la documentazione](docs)

## Unisciti alla comunità

Codewhale migliora quando le persone lo usano, segnalano ciò che non funziona e aiutano a correggerlo. Se manca un provider, un workflow risulta scomodo o l’interfaccia del terminale ti ostacola, [apri una issue](https://github.com/Hmbown/CodeWhale/issues). Se sai come migliorarlo, [apri una pull request](CONTRIBUTING.md). I primi contributi sono benvenuti e chi contribuisce mantiene il riconoscimento per il lavoro integrato.

Unisciti a [Discord](https://discord.gg/37gfS3ksug), oppure aggiungi Hunter su WeChat (`hunterbown`) e chiedi di entrare nel gruppo Whale Brothers.

## Storia del progetto

Codewhale è nato come `deepseek-tui` e conserva ancora la compatibilità con la sua configurazione e le sue sessioni. Ora è indipendente dai provider, viene mantenuto in modo autonomo e non è affiliato ad alcun provider di modelli.

Grazie a ogni persona che ha contribuito e alle comunità open source che hanno aiutato il progetto a crescere. Consulta il [registro dei contributori](docs/CONTRIBUTORS.md).

## Licenza

[MIT](LICENSE). Le parti adattate da altri progetti open source sono indicate nelle [note sui componenti di terze parti](docs/THIRD_PARTY_NOTICES.md).
