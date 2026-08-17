<!-- source: README.md sha256:4fc19c5f9596 -->
# Codewhale

Un agent de programació de codi obert per al teu terminal — porta el teu propi model.

El Codewhale va començar com una experiència nativa per al DeepSeek. Des d'aleshores
s'ha convertit en un projecte impulsat per la comunitat: un harness de programació
que encaixa amb una comunitat internacional en creixement i admet tants models i
proveïdors com sigui possible — els models oberts primer, allotjats o locals, sense
privilegiar-ne cap.

Dona-li un proveïdor, un model i una tasca. Llegeix el teu codi, edita fitxers,
executa ordres i comprova la seva pròpia feina, i s'atura quan la tasca s'ha acabat
o et necessita. Canvia de model a mig camí amb `/model`. Treballa de forma
interactiva a la TUI, o executa `codewhale exec` en scripts i CI. Està escrit en
Rust, amb llicència MIT, i corre a la teva màquina.

El que no s'assembla als altres harnessos: **tu tries el model de cada rol, i no
cal que coincideixin.** Una fleet fixa un proveïdor, un model i un nivell de
raonament per rol — així un model barat i ràpid pot dirigir-ne un de raonament car,
o un builder GLM pot treballar en la mateixa tasca que un reviewer Kimi. Escriu els
teus propis rols i la teva pròpia constitution, i el harness és teu en lloc de nostre.

Sempre busquem persones que hi contribueixin i maneres de millorar. Si falta un
model o un proveïdor que uses, o alguna cosa es trenca, dir-nos-ho és una de les
coses més útils que pots fer — mira [Contribuir](#contribuir).

[English](README.md) · [简体中文](README.zh-CN.md) · [日本語](README.ja-JP.md) · [Tiếng Việt](README.vi.md) · [Bahasa Indonesia](README.id.md) · [한국어](README.ko-KR.md) · [Español](README.es-419.md) · [Português](README.pt-BR.md) · [Русский](README.ru.md) · [Українська](README.uk.md) · [Français](README.fr.md) · [Deutsch](README.de.md) · [繁體中文](README.zh-TW.md) · [हिन्दी](README.hi.md) · [Türkçe](README.tr.md) · [Italiano](README.it.md) · [Polski](README.pl.md) · [العربية](README.ar.md) · [codewhale.net](https://codewhale.net/) · [Docs](docs) · [Changelog](CHANGELOG.md) · [Discord](https://discord.gg/37gfS3ksug)

[![CI](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml/badge.svg)](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/codewhale-cli?label=crates.io)](https://crates.io/crates/codewhale-cli)
[![npm](https://img.shields.io/npm/v/codewhale?label=npm)](https://www.npmjs.com/package/codewhale)
[![Discord](https://img.shields.io/badge/Discord-join%20the%20community-5865F2?logo=discord&logoColor=white)](https://discord.gg/37gfS3ksug)

![Codewhale en execució en un terminal](assets/screenshot.png)

## Instal·lació

```bash
npm install -g codewhale
```

Cargo, Docker, Nix, Scoop, arxius precompilats, Android/Termux i un mirall CNB
per a qui no pot arribar a GitHub són a
[docs/INSTALL.md](docs/INSTALL.md). Vens de `deepseek-tui`? La configuració i
les sessions es conserven — mira [docs/REBRAND.md](docs/REBRAND.md).

## Ús

```bash
codewhale auth set --provider deepseek   # or export ANTHROPIC_API_KEY, etc.
codewhale                                # open the TUI
codewhale exec "fix the failing test"    # headless
codewhale web                            # local browser client on 127.0.0.1
```

A la TUI: `/model` canvia proveïdor i model alhora, `/fleet` construeix i
executa l'equip — un rol cada vegada, cadascun amb el seu model —, `/undo`
desfà l'últim torn i `/restore <N>` torna l'espai de treball a una instantània
anterior (`/restore` sol els llista). `Tab` recorre Plan / Work / Operate quan
el compositor és buit — amb text, `Tab` completa ordres slash i mencions `@`.
`Shift+Tab` recorre en qualsevol moment la postura de permís Ask / Auto-Review /
Full Access. `!` executa una ordre de l'intèrpret pel camí d'aprovació habitual.

## Què fa

- **Qualsevol model, qualsevol proveïdor — i qualsevol barreja.** DeepSeek,
  Claude, GPT, Kimi, GLM i més de 30 proveïdors, a més del teu vLLM, SGLang o
  Ollama sense clau, tot a través d'un sol runtime i un sol conjunt d'eines. El
  catàleg segueix la línia en viu de cada proveïdor — el backend V4 Pro de
  DeepSeek (etiquetat `DeepSeek-V4-Pro-0813`) continua sent invocable com
  `deepseek-v4-pro`, Grok 4.6 és el predeterminat directe de xAI i OrcaRouter
  enruta per `orcarouter/auto`. Un rol desat registra explícitament el
  `provider`, el `model` i el nivell de raonament, de manera que una fleet pot
  abastar diversos proveïdors en una sola execució i la ruta d'un rol no depèn
  mai de quin proveïdor passa a estar actiu. Els límits de context i els preus
  venen de la ruta real; un preu desconegut es mostra com a desconegut, no com
  0 $.
- **Un harness que escrius tu.** Els rols són fitxers que pots llegir i editar
  — un model, una postura d'eines i instruccions permanents per rol — guardats
  al projecte perquè l'equip els comparteixi, o al costat dels altres ajustos
  personals perquè et segueixin entre repositoris. Una constitution registra com
  vols que l'agent es comporti a cada sessió, de manera que el harness segueixi
  la teva pràctica en lloc de la nostra.
- **Només lectura fins que permetis més.** El mode Plan no pot canviar fitxers,
  i les aprovacions tanquen les ordres arriscades. Quan un sandbox del sistema
  operatiu envolta de debò una ordre, el Codewhale ho diu: Seatbelt a macOS
  quan està disponible, bubblewrap opcional a Linux. El `constitution.json` d'un
  repositori es compila en bloquejos d'escriptura que ni tan sols Full Access
  pot saltar-se.
- **Feina que pots reprendre.** Una fleet registra cada pas en un llibre major
  de només afegir, així que `fleet resume` continua on et vas aturar.

## Integracions

- **DeepSeek Harness (dsh) — connectat a través de Codewhale.**
  `codewhale integrations dsh connect` enllaça una instal·lació existent de
  `@deepseek-ai/dsh` a la teva ruta de proveïdor, permisos i espai de treball
  de Codewhale, i `integrations dsh install-bundle` afegeix el paquet de
  connectors DSH opcional perquè `dsh --profile codewhale` porti aquesta
  identitat tot sol. El Codewhale té els permisos i l'autoritat del cicle de
  vida; el dsh conserva les seves sessions, perfils i credencials intactes.
  Mira [docs/INTEGRATIONS_DSH.md](docs/INTEGRATIONS_DSH.md).
- **VS Code.** L'entramat oficial de l'extensió (`extensions/vscode`) obre el
  Codewhale en un terminal integrat i exposa una Agent View de només lectura
  sobre el runtime local. És una previsualització de desenvolupament local, no
  encara una publicació al marketplace.

## Per saber-ne més

- [docs/PROVIDERS.md](docs/PROVIDERS.md) — cada ruta de proveïdor: allotjada,
  passarel·la i local
- [docs/FLEET.md](docs/FLEET.md) — les fleets, el llibre major i la represa
- [docs/WORKFLOW_EXPERIMENTAL_SEARCH.md](docs/WORKFLOW_EXPERIMENTAL_SEARCH.md) — cerca experimental congelada i neutral respecte al proveïdor dins de Workflow
- [docs/CONFIGURATION.md](docs/CONFIGURATION.md) — `config.toml`, ganxos i la
  constitution
- [docs/AUTHORIZATION_ORDER.md](docs/AUTHORIZATION_ORDER.md) — com es
  combinen modes, ganxos, regles de permís, terres de seguretat, llei del
  repositori, aprovacions i sandbox
- [docs/HOOKS.md](docs/HOOKS.md) — els onze esdeveniments de ganxo del cicle
  de vida de la TUI, les seves càrregues i quins tres poden orientar un torn
  (`codewhale exec` i les subordres de la CLI no disparen ganxos)
- [docs/WEB.md](docs/WEB.md) — el client de navegador només en loopback i el
  seu límit d'autenticació d'un sol ús

Tota la resta — modes, dreceres, detalls del sandbox, MCP, l'API del runtime
i l'arquitectura — viu a [docs](docs) i a
[codewhale.net](https://codewhale.net/).

## Contribuir

Els issues, els PR, els passos de reproducció, els registres i les peticions
de funcionalitat són feina real de projecte, i les primeres contribucions són
benvingudes. Quan un PR no es pot fusionar tal com està, els mantenidors
recullen el que funciona i l'autor resta acreditat — al commit, al changelog i
a [docs/CONTRIBUTORS.md](docs/CONTRIBUTORS.md).

- [Issues oberts](https://github.com/Hmbown/CodeWhale/issues) — les bones
  primeres contribucions són aquí
- [CONTRIBUTING.md](CONTRIBUTING.md) — configuració de desenvolupament i flux
  de PR
- [docs/CONTRIBUTORS.md](docs/CONTRIBUTORS.md) — tothom qui ha donat forma a
  això
- [Convida'm a un cafè](https://www.buymeacoffee.com/hmbown)

Gràcies a [DeepSeek](https://github.com/deepseek-ai) pels models i el suport
que van iniciar el projecte, a [DataWhale](https://github.com/datawhalechina)
🐋 per acollir-nos a la família Whale Brother, i a
[OpenWarp](https://github.com/zerx-lab/warp) i
[Open Design](https://github.com/nexu-io/open-design) per col·laborar en
l'experiència d'agent al terminal.

## Llicència

[MIT](LICENSE). Projecte comunitari independent, no afiliat a cap proveïdor de
models.

![Codewhale desplegant tres subagents scout de només lectura en un terminal](assets/fanout.gif)
