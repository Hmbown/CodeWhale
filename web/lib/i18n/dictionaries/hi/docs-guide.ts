import type { DocsGuideDict } from "../types";

/**
 * Hindi dictionary for the docs "Getting started" page. Devanagari gets a
 * touch more leading than the Latin reference, short of the CJK treatment.
 */
export const docsGuide: DocsGuideDict = {
  metaTitle: "शुरुआत कैसे करें · Codewhale दस्तावेज़ीकरण",
  metaDescription:
    "इंस्टॉल से लेकर आपकी आदर्श fleet तक का पूरा रास्ता: इंस्टॉल, बिना कुंजी पहला सेशन, प्रोवाइडर कनेक्शन और fleet सेटअप।",
  bodyClassName: "text-ink-soft leading-loose",
  overviewTitle: "शुरुआत कैसे करें",
  overviewLead:
    "एक इंस्टॉल कमांड से लेकर आपके काम के लिए तैयार fleet तक, चार कदम।",
  sessionTitle: "एक असली सेशन देखें",
  sessionLead:
    "यहाँ एक असली सेशन की रिकॉर्डिंग आएगी। अभी कोई रिकॉर्डिंग नहीं है, इसलिए कुछ नहीं दिखाया गया है।",
  nextTitle: "आगे क्या",
  sourceNote:
    "स्रोत दस्तावेज़: docs/GUIDE.md, docs/KEYBINDINGS.md · कदमों का पाठ web/lib/content/getting-started.ts में है; बदलाव पर docs-map.ts भी अपडेट करें।",
};
