<!-- source: README.md sha256:e421b94e9f7e -->
# Codewhale

Codewhale é um agente de programação de código aberto para o seu terminal, desenvolvido em Rust e aprimorado publicamente com as pessoas que o utilizam.

![Codewhale em execução em um terminal](assets/screenshot.webp)

[English](README.md) · [简体中文](README.zh-CN.md) · [日本語](README.ja-JP.md) · [Tiếng Việt](README.vi.md) · [Bahasa Indonesia](README.id.md) · [한국어](README.ko-KR.md) · [Español](README.es-419.md) · [Русский](README.ru.md) · [Українська](README.uk.md) · [Français](README.fr.md) · [Deutsch](README.de.md) · [繁體中文](README.zh-TW.md) · [हिन्दी](README.hi.md) · [Türkçe](README.tr.md) · [Italiano](README.it.md) · [Polski](README.pl.md) · [العربية](README.ar.md) · [Català](README.ca.md)

[![CI](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml/badge.svg)](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/codewhale-cli?label=crates.io)](https://crates.io/crates/codewhale-cli)
[![npm](https://img.shields.io/npm/v/codewhale?label=npm)](https://www.npmjs.com/package/codewhale)
[![Discord](https://img.shields.io/badge/Discord-join-5865F2?logo=discord&logoColor=white)](https://discord.gg/37gfS3ksug)

## Instalação

```bash
npm install -g codewhale
codewhale
```

Na primeira execução, o Codewhale ajuda você a conectar um provedor ou a continuar offline. Ele também oferece suporte a Cargo, Docker, Nix, Scoop, arquivos pré-compilados, Android/Termux e um espelho CNB. Consulte o [guia de instalação](docs/INSTALL.md).

O preenchimento automático com Tab é ativado com um comando por shell — `codewhale completion bash|zsh|fish|powershell|elvish`. Consulte o [preenchimento automático do shell](docs/INSTALL.md#8-shell-completions).

## Uso

Converse com o Codewhale como você conversaria com alguém da sua equipe:

```text
Fix the failing tests and explain what changed.
```

Ou execute uma tarefa sem abrir a TUI:

```bash
codewhale exec "fix the failing tests and explain what changed"
```

O Codewhale pode ler seu repositório, editar arquivos, executar comandos, verificar resultados e continuar trabalhando em direção a um objetivo. Você decide quanto acesso ele terá.

## Por que usar o Codewhale

- **Use o modelo que quiser.** Conecte provedores hospedados ou modelos locais por meio do Ollama, vLLM ou SGLang. Alterne o provedor e o modelo com `/model`.
- **Mantenha o controle.** O modo Plan é somente leitura. Ask, Auto-Review e Full Access tornam visível o comportamento das aprovações. `/undo` desfaz o último turno e `/restore` retorna o espaço de trabalho a um snapshot anterior.
- **Mantenha trabalhos longos organizados.** Salve sessões, defina um `/goal` duradouro, revise os fluxos de trabalho antes da execução e coordene agentes sem transformar as instruções internas deles em parte da sua conversa.
- **Amplie o agente que você já tem.** Conecte servidores MCP e habilidades, configure hooks e mantenha as funções dos agentes como arquivos legíveis no projeto ou nas suas configurações pessoais.

Execute `/help` na TUI para ver comandos e atalhos de teclado.

## Segurança

O Codewhale é executado na sua máquina com o acesso que você conceder. Os modos de aprovação e as regras do repositório limitam o que o agente pode fazer; o sandbox opcional do sistema operacional adiciona um limite de execução mais forte quando disponível. Preços de modelos desconhecidos continuam sendo mostrados como desconhecidos, em vez de serem informados como gratuitos.

Leia a [ordem de autorização](docs/AUTHORIZATION_ORDER.md) para conhecer a hierarquia exata das políticas e a [configuração](docs/CONFIGURATION.md) para os ajustes locais.

## Documentação

- [Provedores e modelos locais](docs/PROVIDERS.md)
- [Equipes de agentes](docs/FLEET.md)
- [MCP](docs/MCP.md), [hooks](docs/HOOKS.md) e [configuração](docs/CONFIGURATION.md)
- [Cliente web local](docs/WEB.md)
- [Toda a documentação](docs)

## Participe da comunidade

O Codewhale melhora quando as pessoas o utilizam, relatam o que parece errado e ajudam a corrigir. Se estiver faltando um provedor, se um fluxo de trabalho for inconveniente ou se a interface do terminal atrapalhar, [abra uma issue](https://github.com/Hmbown/CodeWhale/issues). Se souber como melhorar, [abra um pull request](CONTRIBUTING.md). Primeiras contribuições são bem-vindas, e os contribuidores mantêm o crédito pelo trabalho incorporado ao projeto.

Participe do [Discord](https://discord.gg/37gfS3ksug), ou adicione Hunter no WeChat (`hunterbown`) e peça para entrar no grupo Whale Brothers.

## História do projeto

O Codewhale começou como `deepseek-tui` e ainda preserva a compatibilidade com as configurações e sessões desse projeto. Hoje ele é neutro em relação a provedores, mantido de forma independente e não tem afiliação com nenhum provedor de modelos.

Agradecemos a cada contribuidor e às comunidades de código aberto que ajudaram o projeto a crescer. Consulte o [registro de contribuidores](docs/CONTRIBUTORS.md).

## Licença

[MIT](LICENSE). As partes adaptadas de outros projetos de código aberto estão registradas nos [avisos de terceiros](docs/THIRD_PARTY_NOTICES.md).
