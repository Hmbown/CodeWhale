import type { DocsGuideDict } from "../types";

/**
 * Hindi dictionary for the docs "Getting started" page. Devanagari gets a
 * touch more leading than the Latin reference, short of the CJK treatment.
 */
export const docsGuide: DocsGuideDict = {
  metaTitle: "शुरुआत कैसे करें · Codewhale दस्तावेज़ीकरण",
  metaDescription:
    "इंस्टॉल से लेकर आपकी आदर्श Fleet तक का पूरा रास्ता: इंस्टॉल, बिना कुंजी पहला सेशन, प्रोवाइडर कनेक्शन और Fleet सेटअप।",
  bodyClassName: "text-ink-soft leading-loose",
  overviewTitle: "शुरुआत कैसे करें",
  overviewLead:
    "एक इंस्टॉल कमांड से लेकर आपके काम के लिए तैयार Fleet तक, चार कदम। हर कदम केवल वही बताता है जो वर्तमान कैंडिडेट वास्तव में करता है; जो अप्रकाशित या अभिलिखित-नहीं है, वह स्पष्ट रूप से चिह्नित है।",
  sessionTitle: "एक असली सेशन देखें",
  sessionLead:
    "नीचे असली-सेशन मीडिया की जगह है। यह जानबूझकर प्रतीक्षारत स्थिति में है: जब तक v0.9.2 कैंडिडेट की dogfood रिकॉर्डिंग मौजूद नहीं है, यह साइट कोई प्लेसहोल्डर या नकली फ़ुटेज नहीं दिखाती।",
  nextTitle: "आगे क्या",
  sourceNote:
    "स्रोत दस्तावेज़: docs/GUIDE.md, docs/KEYBINDINGS.md · कदमों का पाठ web/lib/content/getting-started.ts में है; बदलाव पर docs-map.ts भी अपडेट करें।",
};
