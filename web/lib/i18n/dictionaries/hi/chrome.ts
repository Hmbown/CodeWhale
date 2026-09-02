import type { ChromeDict } from "../types";

/**
 * Hindi chrome dictionary.
 *
 * मानक हिन्दी, तुरंत-बरती तकनीकी शब्दावली के साथ — TUI पैक
 * (crates/tui/locales/hi.json) के रजिस्टर से मेल खाता हुआ: «समीक्षा करें»,
 * «चलाएँ»। अंग्रेज़ी की वर्तमान दिशा दर्शाती मूल पुनर्लेखन — कोई भी मॉडल,
 * आपकी मशीन पर।
 *
 * मोड और अनुमति-मुद्रा (posture) literal रहते हैं (Plan / Work / Operate,
 * Ask / Auto-Review / Full Access); `Runtime`, `fleet`, `TUI` उत्पाद-नाम
 * हैं और ऐसे ही रहते हैं।
 *
 * सेकेंडरी नेविगेशन लेबल हिन्दी मुख्य लेबल के साथ छोटा अंग्रेज़ी साथी
 * रखते हैं — हन जोड़ी अंग्रेज़ी संस्करण का अपना संपादकीय उपकरण है।
 */
export const chrome: ChromeDict = {
  navDocs: "दस्तावेज़ीकरण",
  navStart: "शुरुआत",
  navInstall: "इंस्टॉल",
  navFaq: "सामान्य प्रश्न",
  navCommunity: "समुदाय",
  navContribute: "योगदान",

  navDocsSecondary: "Docs",
  navStartSecondary: "Start",
  navInstallSecondary: "Install",
  navFaqSecondary: "FAQ",
  navCommunitySecondary: "Community",
  navContributeSecondary: "Contribute",

  skipToContent: "मुख्य सामग्री पर जाएँ",

  navPrimaryAria: "मुख्य नेविगेशन",
  navHomeAria: "Codewhale होम",

  installCta: "इंस्टॉल करें →",

  authSignIn: "साइन इन करें",
  authRegister: "रजिस्टर करें",
  authGroupAria: "खाता",

  wordmarkSeal: "深",
  wordmarkTag: "कोई भी मॉडल, आपकी मशीन पर",

  issueLabel: "{date} का अंक",
  dateLocale: "hi-IN",

  starsAria: "GitHub स्टार",
  githubFallback: "GitHub",

  tickerLiveLabel: "लाइव",
  tickerLiveTag: "LIVE",
  tickerMerged: "मर्ज",
  tickerOpened: "खोला गया",
  tickerClosed: "बंद किया गया",
  tickerReleased: "रिलीज़ हुआ",
  tickerFirstContribution: "पहला योगदान",
  tickerBy: "{handle} द्वारा",
  tickerAria: "रिपॉज़िटरी की हालिया गतिविधि",

  traceLabel: "रीज़निंग ट्रेस",
  traceTabsAria: "सेशन के अंश",

  menuOpen: "मेनू खोलें",
  menuClose: "मेनू बंद करें",

  themeAuto: "ऑटो",
  themeLight: "लाइट",
  themeDark: "डार्क",
  themeAria: "दस्तावेज़ीकरण थीम: {mode} (बदलने के लिए क्लिक करें)",
  themeTitle: "दस्तावेज़ीकरण थीम · ऑटो / लाइट / डार्क",

  footerTagline:
    "गहराई में Codewhale उतरता है, आपको नहीं उतरना पड़ता — ओपन सोर्स रनटाइम के लिए दस्तावेज़, सोर्स और समुदाय।",
  footerProduct: "उत्पाद",
  footerProject: "प्रोजेक्ट",
  footerDocs: "दस्तावेज़ीकरण",
  footerGuide: "शुरुआत कैसे करें",
  footerInstall: "इंस्टॉल",
  footerModels: "मॉडल",
  footerRuntime: "Runtime",
  footerFaq: "सामान्य प्रश्न",
  footerIssues: "Issues",
  footerContribute: "योगदान दें",
  footerLicense: "MIT लाइसेंस",
  footerPricing: "मूल्य",
  footerTerms: "सेवा की शर्तें",
  footerPrivacy: "गोपनीयता",
  footerChangelog: "परिवर्तन लॉग",
  footerCanonicalSource: "कैननिकल सोर्स: ",
  footerReleases: " · रिलीज़: ",
  footerReleasesLink: "GitHub रिलीज़",
  footerSecurity: "सुरक्षा",

  switcherLabel: "भाषा",
  switcherSwitchTo: "{label} पर स्विच करें",
  partialBadge: "(आंशिक)",
};
