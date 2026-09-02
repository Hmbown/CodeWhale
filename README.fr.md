<!-- source: README.md sha256:e421b94e9f7e -->
# Codewhale

Codewhale est un agent de programmation open source pour votre terminal, développé en Rust et amélioré publiquement avec les personnes qui l’utilisent.

![Codewhale en cours d’exécution dans un terminal](assets/screenshot.webp)

[English](README.md) · [简体中文](README.zh-CN.md) · [日本語](README.ja-JP.md) · [Tiếng Việt](README.vi.md) · [Bahasa Indonesia](README.id.md) · [한국어](README.ko-KR.md) · [Español](README.es-419.md) · [Português](README.pt-BR.md) · [Русский](README.ru.md) · [Українська](README.uk.md) · [Deutsch](README.de.md) · [繁體中文](README.zh-TW.md) · [हिन्दी](README.hi.md) · [Türkçe](README.tr.md) · [Italiano](README.it.md) · [Polski](README.pl.md) · [العربية](README.ar.md) · [Català](README.ca.md)

[![CI](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml/badge.svg)](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/codewhale-cli?label=crates.io)](https://crates.io/crates/codewhale-cli)
[![npm](https://img.shields.io/npm/v/codewhale?label=npm)](https://www.npmjs.com/package/codewhale)
[![Discord](https://img.shields.io/badge/Discord-join-5865F2?logo=discord&logoColor=white)](https://discord.gg/37gfS3ksug)

## Installation

```bash
npm install -g codewhale
codewhale
```

Au premier lancement, Codewhale vous aide à connecter un fournisseur ou à rester hors ligne. Il prend également en charge Cargo, Docker, Nix, Scoop, les archives précompilées, Android/Termux et un miroir CNB. Consultez le [guide d’installation](docs/INSTALL.md).

L’autocomplétion avec Tab s’active avec une commande par shell — `codewhale completion bash|zsh|fish|powershell|elvish`. Consultez [l’autocomplétion du shell](docs/INSTALL.md#8-shell-completions).

## Utilisation

Parlez à Codewhale comme vous parleriez à un membre de votre équipe :

```text
Fix the failing tests and explain what changed.
```

Vous pouvez aussi exécuter une tâche sans ouvrir la TUI :

```bash
codewhale exec "fix the failing tests and explain what changed"
```

Codewhale peut lire votre dépôt, modifier des fichiers, exécuter des commandes, inspecter les résultats et continuer à travailler vers un objectif. Vous choisissez le niveau d’accès que vous lui accordez.

## Pourquoi Codewhale

- **Utilisez le modèle de votre choix.** Connectez des fournisseurs hébergés ou des modèles locaux via Ollama, vLLM ou SGLang. Changez de fournisseur et de modèle avec `/model`.
- **Gardez le contrôle.** Le mode Plan est en lecture seule. Ask, Auto-Review et Full Access rendent le comportement des approbations explicite. `/undo` annule le dernier tour et `/restore` ramène l’espace de travail à un instantané antérieur.
- **Organisez les travaux de longue durée.** Enregistrez les sessions, définissez un `/goal` durable, examinez les workflows avant leur exécution et coordonnez des agents sans faire apparaître leurs instructions internes dans votre conversation.
- **Étendez l’agent que vous possédez déjà.** Connectez des serveurs MCP et des compétences, configurez des hooks et conservez les rôles d’agent sous forme de fichiers lisibles dans votre projet ou vos paramètres personnels.

Exécutez `/help` dans la TUI pour afficher les commandes et les raccourcis clavier.

## Sécurité

Codewhale s’exécute sur votre machine avec les accès que vous lui accordez. Les modes d’approbation et les règles du dépôt limitent les actions de l’agent ; un bac à sable facultatif du système d’exploitation renforce la limite d’exécution lorsqu’il est pris en charge. Le prix d’un modèle inconnu reste indiqué comme tel au lieu d’être présenté comme gratuit.

Consultez l’[ordre d’autorisation](docs/AUTHORIZATION_ORDER.md) pour connaître la hiérarchie exacte des politiques et la [configuration](docs/CONFIGURATION.md) pour les paramètres locaux.

## Documentation

- [Fournisseurs et modèles locaux](docs/PROVIDERS.md)
- [Équipes d’agents](docs/FLEET.md)
- [MCP](docs/MCP.md), [hooks](docs/HOOKS.md) et [configuration](docs/CONFIGURATION.md)
- [Client web local](docs/WEB.md)
- [Toute la documentation](docs)

## Rejoindre la communauté

Codewhale progresse lorsque les gens l’utilisent, signalent ce qui ne va pas et contribuent aux correctifs. S’il manque un fournisseur, si un workflow est peu pratique ou si l’interface du terminal vous gêne, [ouvrez une issue](https://github.com/Hmbown/CodeWhale/issues). Si vous savez comment l’améliorer, [ouvrez une pull request](CONTRIBUTING.md). Les premières contributions sont les bienvenues, et les personnes qui contribuent restent créditées pour le travail intégré.

Rejoignez le [Discord](https://discord.gg/37gfS3ksug), ou ajoutez Hunter sur WeChat (`hunterbown`) et demandez à rejoindre le groupe Whale Brothers.

## Historique du projet

Codewhale a commencé sous le nom de `deepseek-tui` et conserve la compatibilité avec sa configuration et ses sessions. Il est désormais indépendant de tout fournisseur, maintenu de manière autonome et n’est affilié à aucun fournisseur de modèles.

Merci à toutes les personnes qui contribuent et aux communautés open source qui ont aidé le projet à grandir. Consultez le [registre des contributeurs](docs/CONTRIBUTORS.md).

## Licence

[MIT](LICENSE). Les parties adaptées d’autres projets open source sont répertoriées dans les [mentions relatives aux logiciels tiers](docs/THIRD_PARTY_NOTICES.md).
