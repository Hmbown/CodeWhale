<!-- source: README.md sha256:ddc9cd7b5aea -->
# Codewhale

Um agente de código para o seu terminal. Funciona com qualquer modelo; modelos
abertos em primeiro lugar.

Você informa um provedor, um modelo e uma tarefa. Ele lê código, edita
arquivos, executa comandos, verifica os resultados e continua até a tarefa
terminar ou até precisar de você. TUI para trabalho interativo,
`codewhale exec` para scripts e CI. Rust, MIT, roda inteiramente na sua
máquina.

Começou como `deepseek-tui`. A comunidade que se formou em volta dele
precisava de mais provedores, então hoje DeepSeek, Claude, GPT, Kimi, GLM e
mais de 30 outros rodam pelo mesmo runtime e pelas mesmas ferramentas.

[English](README.md) · [简体中文](README.zh-CN.md) · [日本語](README.ja-JP.md) · [Tiếng Việt](README.vi.md) · [한국어](README.ko-KR.md) · [Español](README.es-419.md) · [codewhale.net](https://codewhale.net/) · [Docs](docs) · [Changelog](CHANGELOG.md)

[![CI](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml/badge.svg)](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/codewhale-cli?label=crates.io)](https://crates.io/crates/codewhale-cli)
[![npm](https://img.shields.io/npm/v/codewhale?label=npm)](https://www.npmjs.com/package/codewhale)

![Codewhale rodando em um terminal](assets/screenshot.png)

## Instalação

```bash
npm install -g codewhale
```

Cargo, Docker, Nix, Scoop, arquivos pré-compilados, Android/Termux e um
espelho CNB para quem não consegue acessar o GitHub estão cobertos em
[docs/INSTALL.md](docs/INSTALL.md). Vindo do `deepseek-tui`? Sua configuração
e suas sessões são preservadas — veja [docs/REBRAND.md](docs/REBRAND.md).

## Uso

```bash
codewhale auth set --provider deepseek   # or export ANTHROPIC_API_KEY, etc.
codewhale                                # open the TUI
codewhale exec "fix the failing test"    # headless
```

Na TUI: `/model` troca provedor e modelo juntos, `/fleet` executa uma equipe
de workers, `/restore` desfaz um turno, `Tab` cicla entre Plan / Act / Operate,
`Shift+Tab` cicla a postura de permissão Ask / Auto-Review / Full Access, e
`!` executa um comando de shell pelo caminho normal de aprovação.

## O que ele faz

- Resolve sua escolha de provedor + modelo em uma rota concreta: endpoint,
  protocolo de comunicação, limite de contexto, preço. Orçamentos de contexto
  e a exibição de custo vêm da rota real; um preço desconhecido aparece como
  desconhecido, não como $0.
  ([docs/PROVIDERS.md](docs/PROVIDERS.md))
- Conversa com provedores hospedados de modelos abertos (`deepseek`,
  `openrouter`, `moonshot`, `zai`, `minimax`, `nvidia-nim`, …), com seu
  próprio `vllm` / `sglang` / `ollama` sem chave, e com a Anthropic
  nativamente pela Messages API, com thinking e cache de prompt.
- Executa vários workers de forma durável: o Fleet registra o trabalho em um
  ledger append-only, então as execuções sobrevivem a reinícios e
  `fleet resume` retoma de onde as coisas pararam. O Workflow planeja
  trabalhos maiores em trilhas retomáveis e verificáveis.
  ([docs/FLEET.md](docs/FLEET.md))
- Controla risco em código, não em achismo: três modos (Plan é somente
  leitura), uma postura de permissão separada, sandbox do sistema operacional
  (Seatbelt, Landlock + seccomp, bwrap), hooks que podem
  permitir/negar/perguntar a cada chamada de ferramenta, e snapshots em um git
  paralelo para que `/restore` nunca toque no seu histórico real.
- Permite que um repositório declare sua própria lei: os invariantes de
  `.codewhale/constitution.json` compilam em bloqueios de escrita que nem o
  Full Access consegue pular.
  ([docs/CONFIGURATION.md](docs/CONFIGURATION.md))
- Fala MCP nas duas direções, carrega skills reutilizáveis, expõe APIs de
  runtime HTTP/SSE e ACP, e dá suporte a uma
  [GUI para VS Code](https://github.com/HengQuWorld/CodeWhale-VSCode) da
  comunidade.
- A TUI mostra o trabalho como recibos que você pode inspecionar, mantém uma
  única linha ao vivo em movimento, tem um inspetor de contexto de verdade,
  12 temas, modos de movimento reduzido e ASCII seguro, e é distribuída em
  English, 简体中文, 日本語, Tiếng Việt, Español, Português, 한국어 e 繁體中文
  parcial.

Todo o resto — configuração, atalhos de teclado, detalhes do sandbox,
arquitetura — está em [docs](docs) e em [codewhale.net](https://codewhale.net/).

## Contribuindo

Todo feedback é um presente. Issues, PRs, passos de reprodução, logs, pedidos
de funcionalidade e primeiras contribuições — tudo isso é trabalho real do
projeto aqui. Quando um PR não pode ser mesclado como está, os mantenedores
aproveitam o que funciona e o autor continua creditado — no commit, no
changelog e em [docs/CONTRIBUTORS.md](docs/CONTRIBUTORS.md). Se um modelo ou
provedor que você usa está faltando, ou se algo quebra na sua máquina, nos
contar é a coisa mais útil que você pode fazer.

- [Issues abertas](https://github.com/Hmbown/CodeWhale/issues) — boas
  primeiras contribuições moram aqui
- [CONTRIBUTING.md](CONTRIBUTING.md) — setup de desenvolvimento e fluxo de PR
- [docs/CONTRIBUTORS.md](docs/CONTRIBUTORS.md) — todo mundo que ajudou a
  moldar o projeto
- [Me pague um café](https://www.buymeacoffee.com/hmbown)

Obrigado à [DeepSeek](https://github.com/deepseek-ai) pelos modelos e pelo
apoio que deram início ao projeto, à
[DataWhale](https://github.com/datawhalechina) 🐋 por nos receber na família
Whale Brother, e a [OpenWarp](https://github.com/zerx-lab/warp) e
[Open Design](https://github.com/nexu-io/open-design) pela colaboração na
experiência de agente no terminal.

## Licença

[MIT](LICENSE). Projeto comunitário independente; sem afiliação com nenhum
provedor de modelos.

[![Gráfico de Star History](https://api.star-history.com/chart?repos=Hmbown/CodeWhale&type=date&legend=top-left)](https://www.star-history.com/?repos=Hmbown%2FCodeWhale&type=date)
