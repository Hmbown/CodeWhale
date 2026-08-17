<!-- source: README.md sha256:4fc19c5f9596 -->
# Codewhale

Un agente di programmazione open source per il tuo terminale — porta il tuo modello.

Codewhale è nato come esperienza nativa per DeepSeek. Da allora è diventato un
progetto guidato dalla comunità: un harness di programmazione che sta a una
comunità internazionale in crescita e supporta quanti più modelli e fornitori
possibile — prima i modelli aperti, ospitati o locali, nessuno privilegiato
rispetto agli altri.

Dagli un fornitore, un modello e un compito. Legge il tuo codice, modifica i
file, esegue comandi e controlla il proprio lavoro, poi si ferma quando il
compito è finito o ha bisogno di te. Cambia modello a metà compito con
`/model`. Lavora in modo interattivo nella TUI, oppure esegui `codewhale exec`
in script e CI. È scritto in Rust, con licenza MIT, e gira sulla tua macchina.

Quello che non assomiglia agli altri harness: **scegli tu il modello di ogni
ruolo, e non devono coincidere.** Una fleet fissa un fornitore, un modello e
un livello di ragionamento per ruolo — così un modello economico e veloce può
dirigere uno di ragionamento costoso, o un builder GLM può lavorare sullo
stesso compito di un reviewer Kimi. Scrivi i tuoi ruoli e la tua constitution,
e l'harness è tuo invece che nostro.

Cerchiamo sempre contributori e modi per migliorare. Se manca un modello o un
fornitore che usi, o qualcosa si rompe, dircelo è una delle cose più utili che
tu possa fare — vedi [Contribuire](#contribuire).

[English](README.md) · [简体中文](README.zh-CN.md) · [日本語](README.ja-JP.md) · [Tiếng Việt](README.vi.md) · [Bahasa Indonesia](README.id.md) · [한국어](README.ko-KR.md) · [Español](README.es-419.md) · [Português](README.pt-BR.md) · [Русский](README.ru.md) · [Українська](README.uk.md) · [Français](README.fr.md) · [Deutsch](README.de.md) · [繁體中文](README.zh-TW.md) · [हिन्दी](README.hi.md) · [Türkçe](README.tr.md) · [Polski](README.pl.md) · [العربية](README.ar.md) · [Català](README.ca.md) · [codewhale.net](https://codewhale.net/) · [Docs](docs) · [Changelog](CHANGELOG.md) · [Discord](https://discord.gg/37gfS3ksug)

[![CI](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml/badge.svg)](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/codewhale-cli?label=crates.io)](https://crates.io/crates/codewhale-cli)
[![npm](https://img.shields.io/npm/v/codewhale?label=npm)](https://www.npmjs.com/package/codewhale)
[![Discord](https://img.shields.io/badge/Discord-join%20the%20community-5865F2?logo=discord&logoColor=white)](https://discord.gg/37gfS3ksug)

![Codewhale in esecuzione in un terminale](assets/screenshot.png)

## Installazione

```bash
npm install -g codewhale
```

Cargo, Docker, Nix, Scoop, archivi precompilati, Android/Termux e uno specchio
CNB per chi non raggiunge GitHub sono in
[docs/INSTALL.md](docs/INSTALL.md). Arrivi da `deepseek-tui`? Configurazione e
sessioni restano — vedi [docs/REBRAND.md](docs/REBRAND.md).

## Uso

```bash
codewhale auth set --provider deepseek   # or export ANTHROPIC_API_KEY, etc.
codewhale                                # open the TUI
codewhale exec "fix the failing test"    # headless
codewhale web                            # local browser client on 127.0.0.1
```

Nella TUI: `/model` cambia fornitore e modello insieme, `/fleet` costruisce e
avvia la squadra — un ruolo alla volta, ciascuno con il proprio modello —,
`/undo` annulla l'ultimo turno e `/restore <N>` riporta lo spazio di lavoro a
uno snapshot precedente (`/restore` da solo li elenca). `Tab` cicla Plan /
Work / Operate quando il compositore è vuoto — con del testo, `Tab` completa
invece i comandi slash e le menzioni `@`. `Shift+Tab` cicla in qualsiasi
momento la postura di permesso Ask / Auto-Review / Full Access. `!` esegue un
comando shell dal percorso di approvazione normale.

## Cosa fa

- **Qualsiasi modello, qualsiasi fornitore — e qualsiasi mix.** DeepSeek,
  Claude, GPT, Kimi, GLM e oltre 30 fornitori, più il tuo vLLM, SGLang o
  Ollama senza chiave, tutto attraverso un solo runtime e un solo set di
  strumenti. Il catalogo segue la lineup dal vivo di ogni fornitore — il
  backend V4 Pro di DeepSeek (etichettato `DeepSeek-V4-Pro-0813`) resta
  chiamabile come `deepseek-v4-pro`, Grok 4.6 è il predefinito xAI diretto e
  OrcaRouter instrada tramite `orcarouter/auto`. Un ruolo salvato registra in
  modo esplicito `provider`, `model` e il livello di ragionamento, così una
  fleet può attraversare più vendor in una sola esecuzione e la rotta di un
  ruolo non dipende mai da quale fornitore capita di essere attivo. Limiti di
  contesto e prezzi arrivano dalla rotta vera; un prezzo sconosciuto si mostra
  come sconosciuto, non come 0 $.
- **Un harness che scrivi tu.** I ruoli sono file che puoi leggere e
  modificare — un modello, una postura degli strumenti e istruzioni permanenti
  per ruolo — tenuti nel progetto perché la squadra li condivida, o accanto
  alle altre impostazioni personali perché ti seguano tra i repo. Una
  constitution registra come vuoi che l'agente si comporti in ogni sessione,
  così l'harness segue la tua pratica invece della nostra.
- **Sola lettura finché non concedi di più.** La modalità Plan non può
  cambiare file e le approvazioni filtrano i comandi rischiosi. Quando una
  sandbox del sistema operativo avvolge davvero un comando, Codewhale lo dice:
  Seatbelt su macOS dove disponibile, bubblewrap opzionale su Linux. Il
  `constitution.json` di un repo si compila in blocchi in scrittura che nemmeno
  Full Access può saltare.
- **Lavoro che puoi riprendere.** Una fleet registra ogni passo in un registro
  in sola appendice, così `fleet resume` riparte da dove ti eri fermato.

## Integrazioni

- **DeepSeek Harness (dsh) — collegato tramite Codewhale.**
  `codewhale integrations dsh connect` collega un'installazione esistente di
  `@deepseek-ai/dsh` alla tua rotta fornitore Codewhale, ai permessi e allo
  spazio di lavoro, e `integrations dsh install-bundle` aggiunge il bundle di
  plugin DSH opzionale perché `dsh --profile codewhale` porti da solo quella
  identità. Codewhale detiene permessi e autorità sul ciclo di vita; dsh
  conserva sessioni, profili e credenziali intatti. Vedi
  [docs/INTEGRATIONS_DSH.md](docs/INTEGRATIONS_DSH.md).
- **VS Code.** Lo scaffold ufficiale dell'estensione (`extensions/vscode`)
  apre Codewhale in un terminale integrato ed espone una Agent View in sola
  lettura sul runtime locale. È un'anteprima di sviluppo locale, non ancora
  una pubblicazione sul marketplace.

## Per saperne di più

- [docs/PROVIDERS.md](docs/PROVIDERS.md) — ogni rotta fornitore: ospitata,
  gateway e locale
- [docs/FLEET.md](docs/FLEET.md) — le fleet, il registro e il ripristino
- [docs/WORKFLOW_EXPERIMENTAL_SEARCH.md](docs/WORKFLOW_EXPERIMENTAL_SEARCH.md) — ricerca sperimentale congelata e neutrale rispetto al fornitore in Workflow
- [docs/CONFIGURATION.md](docs/CONFIGURATION.md) — `config.toml`, hook e la
  constitution
- [docs/AUTHORIZATION_ORDER.md](docs/AUTHORIZATION_ORDER.md) — come si
  compongono modalità, hook, regole di permesso, pavimenti di sicurezza,
  legge del repo, approvazioni e sandbox
- [docs/HOOKS.md](docs/HOOKS.md) — gli undici eventi hook del ciclo di vita
  TUI, i loro payload e quali tre possono orientare un turno (`codewhale exec`
  e i sottocomandi CLI non sparano hook)
- [docs/WEB.md](docs/WEB.md) — il client browser solo in loopback e il suo
  confine di autenticazione monouso

Tutto il resto — modalità, scorciatoie, dettagli della sandbox, MCP, l'API
runtime e l'architettura — vive in [docs](docs) e su
[codewhale.net](https://codewhale.net/).

## Contribuire

Issue, PR, passi di riproduzione, log e richieste di funzionalità sono tutti
lavoro reale di progetto, e i primi contributi sono benvenuti. Quando una PR
non si può unire così com'è, i maintainer raccolgono ciò che funziona e
l'autore resta accreditato — nel commit, nel changelog e in
[docs/CONTRIBUTORS.md](docs/CONTRIBUTORS.md).

- [Issue aperte](https://github.com/Hmbown/CodeWhale/issues) — i buoni primi
  contributi stanno qui
- [CONTRIBUTING.md](CONTRIBUTING.md) — setup di sviluppo e flusso PR
- [docs/CONTRIBUTORS.md](docs/CONTRIBUTORS.md) — chi ha dato forma a questo
- [Offrimi un caffè](https://www.buymeacoffee.com/hmbown)

Grazie a [DeepSeek](https://github.com/deepseek-ai) per i modelli e il
sostegno che hanno avviato il progetto, a
[DataWhale](https://github.com/datawhalechina) 🐋 per averci accolto nella
famiglia Whale Brother, e a [OpenWarp](https://github.com/zerx-lab/warp) e
[Open Design](https://github.com/nexu-io/open-design) per la collaborazione
sull'esperienza di agente nel terminale.

## Licenza

[MIT](LICENSE). Progetto comunitario indipendente, non affiliato ad alcun
fornitore di modelli.

![Codewhale che dispiega tre sottoagenti scout in sola lettura in un terminale](assets/fanout.gif)
