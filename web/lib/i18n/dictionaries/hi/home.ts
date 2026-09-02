import type { HomeDict } from "../types";

/**
 * Hindi home dictionary — «अख़बार-महासागर» लैंडिंग पेज।
 *
 * अंग्रेज़ी की वर्तमान दिशा में मूल पुनर्लेखन: मॉडल अपना लाइए, सब कुछ आपकी
 * मशीन पर। उत्पाद-शब्दावली TUI पैक की तरह literal रहती है: Plan / Work /
 * Operate, Ask / Auto-Review / Full Access, Codewhale, TUI,
 * `codewhale exec`, Runtime API + MCP, fleet, Node 18+, Rust, MIT।
 *
 * सेक्शन-मुहरें (法, 行, …) अंग्रेज़ी संस्करण के साथ साझा ग्लिफ़ हैं —
 * चिह्न, गद्य नहीं।
 */
export const home: HomeDict = {
  metaTitle: "Codewhale — गहराई में यह उतरता है, आपको नहीं।",
  metaDescription:
    "गहराई में Codewhale उतरता है, आपको नहीं उतरना पड़ता — टर्मिनल के लिए ओपन सोर्स कोडिंग एजेंट। अपना मॉडल लाइए। आपकी मशीन पर चलता है। Rust, MIT।",

  kicker: "ओपन सोर्स · अपना मॉडल लाइए · टर्मिनल में चलता है",
  heroTitleA: "Codewhale गहराई में उतरता है",
  heroTitleB: "ताकि आपको न उतरना पड़े।",
  heroIntro:
    "{brand} आपके टर्मिनल के लिए एक ओपन सोर्स कोडिंग एजेंट है। इसे एक मॉडल और एक काम दें — यह आपका कोड पढ़ता है, फ़ाइलें संपादित करता है, अपनी जाँचें खुद चलाता है, और काम पूरा होने पर या आपकी ज़रूरत पड़ने पर रुक जाता है। कोई भी मॉडल लाइए, या उन्हें मिलाइए: हर भूमिका के लिए अलग मॉडल तय करें।",
  install: "इंस्टॉल करें",
  docs: "दस्तावेज़ीकरण",
  copy: "कॉपी करें",
  copied: "कॉपी हो गया ✓",

  installEyebrow: "एक पंक्ति में इंस्टॉल",
  installRequirement: "Node 18+ चाहिए — Rust टूलचेन नहीं",
  installOtherWays: "अन्य तरीके →",

  latestRelease: "नवीनतम रिलीज़ {tag}",
  releaseUnavailable: "रिलीज़ स्थिति उपलब्ध नहीं",
  currentSource: "सोर्स",
  sourceCandidate: "अप्रकाशित",
  providerRoutes: "{count} प्रोवाइडर",
  publishedRelease: "प्रकाशित",
  figcaptionSourceCandidate: "अप्रकाशित",

  shotSession: "वर्तमान सेशन",
  screenshotAlt:
    "Codewhale का वर्तमान टर्मिनल सेशन — Operate मोड, व्हेल, कंपोज़र और फ़ुटर दिख रहे हैं",
  figcaption: "Codewhale का वर्तमान सेशन · Operate मोड · Ask अनुमति-मुद्रा",

  proofHeading: "एक जलमग्न टर्मिनल शेल। कोई भी मॉडल। आपकी मशीन पर।",
  proofBody:
    "जो मॉडल आप पहले से उपयोग करते हैं वही लाइए — होस्टेड, गेटवे या लोकल। Plan / Work / Operate और स्पष्ट अनुमति-मुद्राएँ डाइव को आपके नियंत्रण में रखती हैं।",

  sealDecides: "法",
  decidesEyebrow: "देखिए यह कैसे निर्णय लेता है",
  decidesHeading: "नियम जो ट्रेस में दिखते हैं",
  decidesLede:
    "असली सेशन के अंश — प्रोजेक्ट के रैंक्ड नियम मॉडल की रीज़निंग में दिखाई देते हैं, यह सिर्फ़ लैंडिंग पेज का दावा नहीं है।",

  sealWorkflow: "行",
  workflowHeading: "काम से सत्यापित बदल तक।",
  workflow: [
    ["अवलोकन", "रिपॉज़िटरी, उसके निर्देश और काम पढ़ें।"],
    ["कार्य", "स्पष्ट स्वीकृति-सीमाओं के भीतर फ़ाइलें संपादित करें।"],
    ["सत्यापन", "जाँचें चलाएँ और परिणाम देखें।"],
    ["रिपोर्ट", "एक संक्षिप्त, टिकाऊ रसीद छोड़ें।"],
  ],
  receiptAria: "काम की रसीद का उदाहरण",
  receiptInspect: "रिपॉज़िटरी और निर्देश",
  receiptAct: "चुनी गई अनुमति-मुद्रा से संपादन",
  receiptReport: "जाँचें पास · रसीद सहेजी गई",

  sealStart: "起",
  startHeading: "Codewhale पर नए हैं? शुरू से अंत तक चार कदम।",
  startLede:
    "इंस्टॉल → बिना कुंजी पहला सेशन → प्रोवाइडर जोड़ें → अपनी fleet सेट करें। शब्दावली पेज पर परिभाषित हैं।",
  startGuideLink: "शुरुआती गाइड पढ़ें →",
  startVocabularyLink: "उत्पाद शब्दावली देखें →",

  sealBoundaries: "界",
  boundariesHeadingA: "आपका मॉडल।",
  boundariesHeadingB: "आपकी सीमाएँ।",
  boundariesBody:
    "मॉडल, कार्य-मोड और अनुमति-मुद्रा स्पष्ट रूप से चुनें। अज्ञात लागत अज्ञात ही रहती है, और प्रीव्यू सतहें स्पष्ट रूप से चिह्नित रहती हैं।",
  hostedGatewayLocal: "होस्टेड, गेटवे और लोकल मॉडल",
  planActOperateDesc: "केवल-पढ़ने योजना से स्वायत्त संचालन तक",
  askAutoReviewDesc: "काम के लिए अनुमति-मुद्रा चुनें",
  tuiExecWebDesc: "इंटरैक्टिव और हेडलेस रनटाइम सतहें",

  sealSurfaces: "面",
  surfacesHeading: "रनटाइम को वहीं उपयोग करें जहाँ काम होता है।",
  surfaces: [
    ["TUI", "टर्मिनल में इंटरैक्टिव काम"],
    ["codewhale exec", "स्क्रिप्ट और CI"],
    ["वेब क्लाइंट", "केवल-लूपबैक ब्राउज़र क्लाइंट"],
    ["Runtime API + MCP", "लोकल इंटीग्रेशन"],
    ["fleet", "टिकाऊ मल्टी-एजेंट काम"],
  ],
  runtimeLink: "रनटाइम सतहें और स्थिरता टिप्पणियाँ देखें →",

  installBandHeading: "एक ही कमांड से शुरू करें।",
  binaries: "बाइनरी",
  chinaMirrors: "चीन मिरर",
  installGuideLink: "इंस्टॉल गाइड पढ़ें →",

  sealCommunity: "众",
  communityHeading: "खुले में निर्मित",
  communityBody:
    "MIT-लाइसेंस प्राप्त और रनटाइम, प्रोवाइडर, प्लेटफ़ॉर्म, दस्तावेज़ीकरण और परीक्षणों के योगदानकर्ताओं द्वारा गढ़ा गया।",
  communityLinksAria: "समुदाय लिंक",
  contribute: "योगदान दें",
};
