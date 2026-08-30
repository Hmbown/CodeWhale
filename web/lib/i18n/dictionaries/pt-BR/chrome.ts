import type { ChromeDict } from "../types";

/**
 * Dicionário pt-BR do chrome — reescrita nativa que espelha o inglês atual.
 * Os rótulos primários da navegação ficam em português e os secundários, em
 * inglês curto (o inverso do dispositivo editorial em Han da edição de
 * referência). O selo 深 do wordmark é uma marca compartilhada.
 */
export const chrome: ChromeDict = {
  navDocs: "Documentação",
  navStart: "Começar",
  navInstall: "Instalar",
  navFaq: "Dúvidas",
  navCommunity: "Comunidade",
  navContribute: "Contribuir",

  navDocsSecondary: "Docs",
  navStartSecondary: "Start",
  navInstallSecondary: "Install",
  navFaqSecondary: "FAQ",
  navCommunitySecondary: "Community",
  navContributeSecondary: "Contribute",

  skipToContent: "Pular para o conteúdo principal",


  navPrimaryAria: "Navegação principal",
  navHomeAria: "Início do Codewhale",

  installCta: "Instalar →",

  authSignIn: "Entrar",
  authRegister: "Criar conta",
  authGroupAria: "Conta",

  wordmarkSeal: "深",
  wordmarkTag: "qualquer modelo, na sua máquina",

  issueLabel: "Edição {date}",
  dateLocale: "pt-BR",

  starsAria: "Estrelas no GitHub",
  githubFallback: "GitHub",

  tickerLiveLabel: "Ao vivo",
  tickerLiveTag: "LIVE",
  tickerMerged: "mesclado",
  tickerOpened: "aberto",
  tickerClosed: "fechado",
  tickerReleased: "lançado",
  tickerFirstContribution: "primeira contribuição",
  tickerBy: "por {handle}",
  tickerAria: "Atividade recente do repositório",

  traceLabel: "rastro de raciocínio",
  traceTabsAria: "Trechos da sessão",

  menuOpen: "Abrir menu",
  menuClose: "Fechar menu",

  themeAuto: "auto",
  themeLight: "claro",
  themeDark: "escuro",
  themeAria: "Tema da documentação: {mode} (clique para alternar)",
  themeTitle: "Tema da documentação · auto / claro / escuro",

  footerTagline:
    "O Codewhale mergulha fundo para você não precisar — documentação, código-fonte e comunidade do runtime de código aberto.",
  footerProduct: "Produto",
  footerProject: "Projeto",
  footerDocs: "Documentação",
  footerGuide: "Primeiros passos",
  footerInstall: "Instalação",
  footerModels: "Modelos",
  footerRuntime: "Runtime",
  footerFaq: "Perguntas frequentes",
  footerIssues: "Issues",
  footerContribute: "Contribuir",
  footerLicense: "Licença MIT",
  footerPricing: "Preços",
  footerTerms: "Termos de serviço",
  footerPrivacy: "Privacidade",
  footerChangelog: "Registro de alterações",
  footerCanonicalSource: "Fonte canônica: ",
  footerReleases: " · Lançamentos: ",
  footerReleasesLink: "Lançamentos no GitHub",
  footerSecurity: "Segurança",

  switcherLabel: "Idioma",
  switcherSwitchTo: "Mudar para {label}",
  partialBadge: "(parcial)",
};
