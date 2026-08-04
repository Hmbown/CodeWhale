import type { HomeDict } from "../types";

export const home: HomeDict = {
  metaTitle: "Codewhale — Mergulhe nas profundezas para que você não precise.",
  metaDescription:
    "O Codewhale mergulha nas profundezas para que você não precise — um agente de terminal que coloca a força dos LLMs nas mãos de quem quer construir coisas. Roda na sua máquina. Rust, licença MIT.",

  kicker: "Código aberto · Qualquer modelo · Roda no seu terminal",
  heroTitleA: "Mergulhe nas profundezas",
  heroTitleB: "para que você não precise.",
  heroIntro:
    "O {brand} coloca a força dos LLMs nas mãos de quem quer construir coisas. No seu terminal, ele lê o repositório, edita arquivos, executa verificações e deixa um recibo — sem supor que você já fale a língua do código. Roda na sua máquina; o modelo é um componente selecionável, não o produto.",
  install: "Instalar",
  docs: "Documentação",
  copy: "Copiar",
  copied: "Copiado ✓",

  installEyebrow: "instalação em uma linha",
  installRequirement: "requer Node 18+ — sem toolchain Rust",
  installOtherWays: "outras formas →",

  latestRelease: "Último lançamento {tag}",
  releaseUnavailable: "Status do lançamento indisponível",
  currentSource: "Fonte atual",
  sourceCandidate: "Fonte candidata",
  providerRoutes: "{count} rotas de provedores",
  publishedRelease: "lançamento publicado",
  figcaptionSourceCandidate: "fonte candidata",

  shotSession: "Sessão atual",
  screenshotAlt:
    "Sessão de terminal atual do Codewhale mostrando o modo Operate, a baleia, o compositor e o rodapé",
  figcaption: "Sessão atual do Codewhale · modo Operate · postura de permissão Ask",

  proofHeading: "Um terminal submarino. Neutro quanto ao modelo. Local primeiro.",
  proofBody:
    "Traga o modelo hospedado, de gateway ou local que você já usa. O Codewhale roda na sua máquina e trata o modelo como um componente selecionável — não o produto. Plan / Act / Operate e posturas de permissão explícitas mantêm o mergulho sob seu controle.",

  sealDecides: "法",
  decidesEyebrow: "Veja como ele decide",
  decidesHeading: "Regras que você acompanha no rastro",
  decidesLede:
    "Trechos fiéis de uma sessão real — a hierarquia de regras do projeto é observável no raciocínio do modelo, não uma afirmação numa landing page.",

  sealWorkflow: "行",
  workflowHeading: "Da tarefa à mudança verificada.",
  workflow: [
    ["Inspecionar", "Lê o repositório, suas instruções e a tarefa."],
    ["Agir", "Edita arquivos dentro de limites de aprovação explícitos."],
    ["Verificar", "Executa verificações e inspeciona o resultado."],
    ["Relatar", "Deixa um recibo conciso e duradouro."],
  ],
  receiptAria: "Exemplo de recibo de trabalho",
  receiptInspect: "repositório e instruções",
  receiptAct: "editar sob a postura de permissão escolhida",
  receiptReport: "verificações aprovadas · recibo salvo",

  sealStart: "起",
  startHeading: "Novo no Codewhale? Quatro passos de ponta a ponta.",
  startLede:
    "Instalar → uma primeira sessão sem chaves → conexão com o provedor → um primeiro Workflow da Fleet. Os termos estão definidos na página de vocabulário.",
  startGuideLink: "Ler o guia de primeiros passos →",
  startVocabularyLink: "Ver o vocabulário do produto →",

  sealBoundaries: "界",
  boundariesHeadingA: "Seu modelo.",
  boundariesHeadingB: "Seus limites.",
  boundariesBody:
    "Escolha explicitamente o modelo, o modo de trabalho e a postura de permissão. Custo desconhecido permanece desconhecido, e recursos em prévia continuam rotulados como tal.",
  hostedGatewayLocal: "Modelos hospedados, de gateway e locais",
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
    ["Fleet", "Trabalho multiagente duradouro"],
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
