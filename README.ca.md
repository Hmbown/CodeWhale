<!-- source: README.md sha256:e421b94e9f7e -->
# Codewhale

Codewhale és un agent de programació de codi obert per al terminal, desenvolupat amb Rust i millorat públicament amb les persones que l’utilitzen.

![Codewhale executant-se en un terminal](assets/screenshot.webp)

[English](README.md) · [简体中文](README.zh-CN.md) · [日本語](README.ja-JP.md) · [Tiếng Việt](README.vi.md) · [Bahasa Indonesia](README.id.md) · [한국어](README.ko-KR.md) · [Español](README.es-419.md) · [Português](README.pt-BR.md) · [Русский](README.ru.md) · [Українська](README.uk.md) · [Français](README.fr.md) · [Deutsch](README.de.md) · [繁體中文](README.zh-TW.md) · [हिन्दी](README.hi.md) · [Türkçe](README.tr.md) · [Italiano](README.it.md) · [Polski](README.pl.md) · [العربية](README.ar.md)

[![CI](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml/badge.svg)](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/codewhale-cli?label=crates.io)](https://crates.io/crates/codewhale-cli)
[![npm](https://img.shields.io/npm/v/codewhale?label=npm)](https://www.npmjs.com/package/codewhale)
[![Discord](https://img.shields.io/badge/Discord-join-5865F2?logo=discord&logoColor=white)](https://discord.gg/37gfS3ksug)

## Instal·lació

```bash
npm install -g codewhale
codewhale
```

En la primera execució, Codewhale t’ajuda a connectar un proveïdor o a continuar sense connexió. També admet Cargo, Docker, Nix, Scoop, arxius precompilats, Android/Termux i un mirall CNB. Consulta la [guia d’instal·lació](docs/INSTALL.md).

L’autocompleció amb Tab s’activa amb una sola ordre per shell — `codewhale completion bash|zsh|fish|powershell|elvish`. Consulta [l’autocompleció del shell](docs/INSTALL.md#8-shell-completions).

## Ús

Parla amb Codewhale tal com parlaries amb una persona del teu equip:

```text
Fix the failing tests and explain what changed.
```

També pots executar una tasca sense obrir la TUI:

```bash
codewhale exec "fix the failing tests and explain what changed"
```

Codewhale pot llegir el teu repositori, editar fitxers, executar ordres, inspeccionar els resultats i continuar treballant cap a un objectiu. Tu decideixes quant accés li concedeixes.

## Per què Codewhale

- **Fes servir el model que vulguis.** Connecta proveïdors allotjats o models locals mitjançant Ollama, vLLM o SGLang. Canvia de proveïdor i de model amb `/model`.
- **Mantén el control.** Plan és només de lectura. Ask, Auto-Review i Full Access fan visible el comportament de les aprovacions. `/undo` desfà l’últim torn i `/restore` retorna l’espai de treball a una instantània anterior.
- **Mantén organitzades les feines llargues.** Desa sessions, defineix un `/goal` durador, revisa els fluxos de treball abans que s’executin i coordina agents sense convertir les seves instruccions internes en part de la teva conversa.
- **Amplia l’agent que ja tens.** Connecta servidors MCP i habilitats, configura hooks i conserva els rols d’agent com a fitxers llegibles al projecte o a la configuració personal.

Executa `/help` a la TUI per veure les ordres i les dreceres de teclat.

## Seguretat

Codewhale s’executa a la teva màquina amb l’accés que li concedeixes. Els modes d’aprovació i les regles del repositori limiten què pot fer l’agent; l’aïllament opcional del sistema operatiu afegeix un límit d’execució més sòlid allà on és compatible. Els preus desconeguts dels models continuen indicant-se com a desconeguts en lloc de presentar-se com a gratuïts.

Llegeix l’[ordre d’autorització](docs/AUTHORIZATION_ORDER.md) per conèixer la jerarquia exacta de polítiques i la [configuració](docs/CONFIGURATION.md) per als ajustos locals.

## Documentació

- [Proveïdors i models locals](docs/PROVIDERS.md)
- [Equips d’agents](docs/FLEET.md)
- [MCP](docs/MCP.md), [hooks](docs/HOOKS.md) i [configuració](docs/CONFIGURATION.md)
- [Client web local](docs/WEB.md)
- [Tota la documentació](docs)

## Uneix-te a la comunitat

Codewhale millora quan les persones l’utilitzen, expliquen què no funciona bé i ajuden a corregir-ho. Si falta un proveïdor, un flux de treball és incòmode o la interfície del terminal et dificulta la feina, [obre una incidència](https://github.com/Hmbown/CodeWhale/issues). Si saps com millorar-lo, [obre una pull request](CONTRIBUTING.md). Les primeres contribucions són benvingudes i qui hi contribueix conserva el reconeixement per la feina incorporada.

Uneix-te al [Discord](https://discord.gg/37gfS3ksug), o afegeix Hunter a WeChat (`hunterbown`) i demana entrar al grup Whale Brothers.

## Història del projecte

Codewhale va començar com a `deepseek-tui` i encara manté la compatibilitat amb la seva configuració i les seves sessions. Ara és neutral pel que fa als proveïdors, es manté de manera independent i no està afiliat a cap proveïdor de models.

Gràcies a totes les persones que hi han contribuït i a les comunitats de codi obert que han ajudat el projecte a créixer. Consulta el [registre de col·laboradors](docs/CONTRIBUTORS.md).

## Llicència

[MIT](LICENSE). Les parts adaptades d’altres projectes de codi obert consten als [avisos de tercers](docs/THIRD_PARTY_NOTICES.md).
