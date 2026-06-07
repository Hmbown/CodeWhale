/** en dictionary — minimal, pages carry inline copy */
const en = {
  nav: {
    links: [
      { href: "/install", label: "Install", cn: "安装" },
      { href: "/docs", label: "Docs", cn: "文档" },
      { href: "/feed", label: "Activity", cn: "动态" },
      { href: "/roadmap", label: "Roadmap", cn: "路线" },
      { href: "/contribute", label: "Contribute", cn: "参与" },
    ],
    edition: "Edition",
    online: "API · Online",
    install: "Install →",
    starGitHub: "★ GitHub",
  },
  footer: {
    cols: [
      {
        title: "Product",
        cn: "产品",
        items: [
          { label: "Install", href: "/install" },
          { label: "Documentation", href: "/docs" },
          { label: "Roadmap", href: "/roadmap" },
          { label: "Releases", href: "https://github.com/Hmbown/CodeWhale/releases" },
        ],
      },
      {
        title: "Community",
        cn: "社区",
        items: [
          { label: "Issues", href: "https://github.com/Hmbown/CodeWhale/issues" },
          { label: "Pull Requests", href: "https://github.com/Hmbown/CodeWhale/pulls" },
          { label: "Discussions", href: "https://github.com/Hmbown/CodeWhale/discussions" },
          { label: "Contribute", href: "/contribute" },
        ],
      },
      {
        title: "Resources",
        cn: "资源",
        items: [
          { label: "Activity Feed", href: "/feed" },
          { label: "Code of Conduct", href: "https://github.com/Hmbown/CodeWhale/blob/main/CODE_OF_CONDUCT.md" },
          { label: "Security", href: "https://github.com/Hmbown/CodeWhale/blob/main/SECURITY.md" },
          { label: "License (MIT)", href: "https://github.com/Hmbown/CodeWhale/blob/main/LICENSE" },
        ],
      },
    ],
    tagline:
      "Open-model terminal-native coding agent. DeepSeek V4 is first-class. MIT licensed. Maintained from a small workshop in Texas. Pull requests welcome.",
    crafted: "Made with care",
    poweredBy: "Maintained with DeepSeek V4-Flash",
    mirrors: "Mirrors",
  },
  localeSwitch: { en: "EN", zh: "中文" },
};

export default en;
