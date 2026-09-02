import type { HomeDict } from "../types";

/**
 * Dicionário pt-BR da home — reescrita nativa que espelha a direção atual do
 * inglês (a linha do mergulho; o posicionamento antigo ficou para trás).
 * Os glifos `seal*` são marcas editoriais compartilhadas entre as
 * localidades, como na edição de referência.
 */
export const home: HomeDict = {
  metaTitle: "Codewhale — mergulha fundo para você não precisar.",
  metaDescription:
    "O Codewhale mergulha fundo para você não precisar — um agente de código open source para o terminal. Traga seu próprio modelo. Roda na sua máquina. Rust, MIT.",

  kicker: "Código aberto · Traga seu próprio modelo · Roda no seu terminal",
  heroTitleA: "O Codewhale mergulha fundo",
  heroTitleB: "para você não precisar.",
  heroIntro:
    "O {brand} é um agente de código open source para o seu terminal. Dê a ele um modelo e uma tarefa — ele lê o seu código, edita arquivos, roda as próprias verificações e para quando o trabalho está pronto ou quando precisa de você. Traga qualquer modelo, ou misture vários: defina um modelo diferente para cada função.",
  install: "Instalar",
  docs: "Documentação",
  copy: "Copiar",
  copied: "Copiado ✓",

  installEyebrow: "instalação em uma linha",
  installRequirement: "precisa de Node 18+ — sem toolchain Rust",
  installOtherWays: "outras formas →",

  latestRelease: "Último lançamento {tag}",
  releaseUnavailable: "Status do lançamento indisponível",
  currentSource: "Código-fonte",
  sourceCandidate: "Não publicado",
  providerRoutes: "{count} provedores",
  publishedRelease: "publicado",
  figcaptionSourceCandidate: "não publicado",

  shotSession: "Sessão atual",
  screenshotAlt:
    "Sessão de terminal atual do Codewhale, mostrando o modo Operate, a baleia, o composer e o rodapé",
  figcaption: "Sessão atual do Codewhale · modo Operate · postura de permissão Ask",

  proofHeading: "Um terminal debaixo d'água. Qualquer modelo. Na sua máquina.",
  proofBody:
    "Traga o modelo que você já usa — hospedado, via gateway ou local. Plan / Work / Operate e posturas de permissão explícitas mantêm o mergulho sob o seu controle.",

  sealDecides: "法",
  decidesEyebrow: "Veja como ele decide",
  decidesHeading: "Regras que você acompanha no rastro",
  decidesLede:
    "Trechos de sessões reais — as regras do projeto, em ordem de prioridade, visíveis no raciocínio do modelo. Não é só promessa de landing page.",

  sealWorkflow: "行",
  workflowHeading: "Da tarefa à mudança verificada.",
  workflow: [
    ["Inspecionar", "Lê o repositório, as instruções e a tarefa."],
    ["Agir", "Edita arquivos dentro de limites de aprovação explícitos."],
    ["Verificar", "Roda as verificações e inspeciona o resultado."],
    ["Relatar", "Deixa um recibo conciso e duradouro."],
  ],
  receiptAria: "Exemplo de recibo de trabalho",
  receiptInspect: "repositório e instruções",
  receiptAct: "edição sob a postura de permissão escolhida",
  receiptReport: "verificações aprovadas · recibo salvo",

  sealStart: "起",
  startHeading: "Novo no Codewhale? Quatro passos de ponta a ponta.",
  startLede:
    "Instalação → primeira sessão sem chave → conectar um provedor → primeiro workflow com a fleet. Os termos estão definidos na página de vocabulário.",
  startGuideLink: "Ler o guia de primeiros passos →",
  startVocabularyLink: "Ver o vocabulário do produto →",

  sealBoundaries: "界",
  boundariesHeadingA: "Seu modelo.",
  boundariesHeadingB: "Seus limites.",
  boundariesBody:
    "Escolha explicitamente o modelo, o modo de trabalho e a postura de permissão. Custo desconhecido continua desconhecido, e recursos em prévia seguem rotulados como tal.",
  hostedGatewayLocal: "Modelos hospedados, via gateway e locais",
  planActOperateDesc: "Do planejamento somente leitura à operação autônoma",
  askAutoReviewDesc: "Escolha a postura de permissão para o trabalho",
  tuiExecWebDesc: "Interfaces de runtime interativas e headless",

  sealSurfaces: "面",
  surfacesHeading: "Use o runtime onde o trabalho acontece.",
  surfaces: [
    ["TUI", "Trabalho interativo no terminal"],
    ["codewhale exec", "Scripts e CI"],
    ["Cliente web", "Cliente de navegador, somente loopback"],
    ["Runtime API + MCP", "Integrações locais"],
    ["fleet", "Trabalho multiagente duradouro"],
  ],
  runtimeLink: "Ver interfaces de runtime e notas de estabilidade →",

  installBandHeading: "Comece com um comando.",
  binaries: "Binários",
  chinaMirrors: "Espelhos da China",
  installGuideLink: "Ler o guia de instalação →",

  sealCommunity: "众",
  communityHeading: "Construído em público",
  communityBody:
    "Licenciado sob MIT e moldado por contribuidores em runtimes, provedores, plataformas, documentação e testes.",
  communityLinksAria: "Links da comunidade",
  contribute: "Contribuir",
};
