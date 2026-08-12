<!-- source: README.md sha256:4fb18fffb0fe -->
# Codewhale

Um agente de programação de código aberto para o seu terminal — traga o seu próprio modelo.

O Codewhale começou como uma experiência nativa para o DeepSeek. Desde então,
virou um projeto guiado pela comunidade: um harness de programação que se
encaixa em uma comunidade internacional em crescimento e suporta o máximo de
modelos e provedores possível — modelos abertos primeiro, hospedados ou locais,
sem privilegiar nenhum.

Você informa um provedor, um modelo e uma tarefa. Ele lê seu código, edita
arquivos, executa comandos e verifica o próprio trabalho, e para quando a tarefa
termina ou quando precisa de você. Troque de modelo no meio da tarefa com
`/model`. Trabalhe de forma interativa na TUI, ou rode `codewhale exec` em
scripts e CI. É escrito em Rust, licenciado sob MIT, e roda na sua máquina.

O que não se parece com outros harnesses: **você escolhe o modelo de cada
papel, e eles não precisam ser iguais.** Uma fleet fixa um provedor, um modelo e
um nível de raciocínio por papel — então um modelo barato e rápido pode dirigir
um modelo de raciocínio caro, ou um builder GLM pode trabalhar na mesma tarefa
que um reviewer Kimi. Escreva seus próprios papéis e sua própria constitution, e
o harness passa a ser seu, não nosso.

Estamos sempre em busca de pessoas que contribuam e de formas de melhorar. Se um
modelo ou provedor que você usa está faltando, ou se algo quebra, nos contar é
uma das coisas mais úteis que você pode fazer — veja [Contribuindo](#contribuindo).

[English](README.md) · [简体中文](README.zh-CN.md) · [日本語](README.ja-JP.md) · [Tiếng Việt](README.vi.md) · [Bahasa Indonesia](README.id.md) · [한국어](README.ko-KR.md) · [Español](README.es-419.md) · [Русский](README.ru.md) · [Українська](README.uk.md) · [codewhale.net](https://codewhale.net/) · [Docs](docs) · [Changelog](CHANGELOG.md) · [Discord](https://discord.gg/37gfS3ksug)

[![CI](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml/badge.svg)](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/codewhale-cli?label=crates.io)](https://crates.io/crates/codewhale-cli)
[![npm](https://img.shields.io/npm/v/codewhale?label=npm)](https://www.npmjs.com/package/codewhale)
[![Discord](https://img.shields.io/badge/Discord-join%20the%20community-5865F2?logo=discord&logoColor=white)](https://discord.gg/37gfS3ksug)

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
codewhale web                            # local browser client on 127.0.0.1
```


Na TUI: `/model` troca provedor e modelo juntos, `/fleet` executa uma equipe
de workers, `/undo` desfaz o último turno e `/restore <N>` reverte o workspace
para um snapshot anterior (`/restore` sem argumento apenas os lista). Quando o
compositor está vazio, `Tab` cicla entre Plan / Work / Operate; com texto
digitado, `Tab` completa comandos slash e menções `@`. `Shift+Tab` cicla a
postura de permissão Ask / Auto-Review / Full Access a qualquer momento. `!`
executa um comando de shell pelo caminho normal de aprovação.

## O que faz

- **Qualquer modelo, qualquer provedor.** DeepSeek, Claude, GPT, Kimi, GLM e
  mais de 30 provedores, além do seu próprio vLLM, SGLang ou Ollama sem key —
  tudo por um único runtime e um único conjunto de ferramentas. Orçamentos de
  contexto e preços vêm da rota real, e um preço desconhecido aparece como
  desconhecido em vez de $0.
- **Um harness escrito por você.** Papéis são arquivos que você pode ler e
  editar — um modelo, uma postura de ferramentas e instruções permanentes por
  papel — guardados no projeto para o time compartilhar, ou ao lado das suas
  configurações pessoais para acompanharem você entre repositórios. Uma
  constitution registra como você quer que o agente se comporte em cada sessão,
  para que o harness siga a sua prática, e não a nossa.
- **Somente leitura até você permitir mais.** O modo Plan não altera arquivos,
  e as aprovações controlam os comandos arriscados. Quando um sandbox do
  sistema operacional realmente envolve um comando, o Codewhale avisa: Seatbelt
  no macOS quando disponível, bubblewrap opcional no Linux. O
  `constitution.json` de um repositório é compilado em bloqueios de escrita
  que nem o Full Access consegue pular.
- **Trabalho que você pode retomar.** Um fleet registra cada passo em um
  livro-razão de apenas inclusão, então `fleet resume` retoma de onde você
  parou.

## Saiba mais

- [docs/PROVIDERS.md](docs/PROVIDERS.md) — cada rota de provedor: hospedada,
  gateway e local
- [docs/FLEET.md](docs/FLEET.md) — fleets, o livro-razão e resume
- [docs/WORKFLOW_EXPERIMENTAL_SEARCH.md](docs/WORKFLOW_EXPERIMENTAL_SEARCH.md) —
  busca experimental congelada e neutra em relação a provedores dentro do
  Workflow
- [docs/CONFIGURATION.md](docs/CONFIGURATION.md) — `config.toml`, hooks e a
  constitution
- [docs/AUTHORIZATION_ORDER.md](docs/AUTHORIZATION_ORDER.md) — como modos, hooks,
  regras de permissão, limites mínimos de segurança, regras do repositório,
  aprovações e sandboxing se combinam
- [docs/HOOKS.md](docs/HOOKS.md) — os onze eventos de hook do ciclo de vida da
  TUI, seus payloads e os três que podem direcionar um turno (`codewhale exec`
  e os subcomandos da CLI não disparam hooks)
- [docs/WEB.md](docs/WEB.md) — cliente de navegador incorporado apenas em
  loopback e sua fronteira de autenticação de uso único

Todo o resto — modos, atalhos de teclado, detalhes do sandbox, MCP, a API do
runtime, arquitetura — está em [docs](docs) e em
[codewhale.net](https://codewhale.net/).

## Contribuindo

Issues, PRs, passos de reprodução, logs e pedidos de funcionalidade são trabalho
real do projeto, e primeiras contribuições são bem-vindas. Quando um PR não pode
ser mesclado como está, os mantenedores aproveitam o que funciona e o autor
continua creditado — no commit, no changelog e em
[docs/CONTRIBUTORS.md](docs/CONTRIBUTORS.md).

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
