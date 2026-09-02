<!-- source: README.md sha256:e421b94e9f7e -->
# Codewhale

Codewhale आपके टर्मिनल के लिए Rust में बना एक ओपन सोर्स कोडिंग एजेंट है, जिसे इसके उपयोगकर्ताओं के साथ सार्वजनिक रूप से बेहतर बनाया जाता है।

![टर्मिनल में चलता Codewhale](assets/screenshot.webp)

[English](README.md) · [简体中文](README.zh-CN.md) · [日本語](README.ja-JP.md) · [Tiếng Việt](README.vi.md) · [Bahasa Indonesia](README.id.md) · [한국어](README.ko-KR.md) · [Español](README.es-419.md) · [Português](README.pt-BR.md) · [Русский](README.ru.md) · [Українська](README.uk.md) · [Français](README.fr.md) · [Deutsch](README.de.md) · [繁體中文](README.zh-TW.md) · [Türkçe](README.tr.md) · [Italiano](README.it.md) · [Polski](README.pl.md) · [العربية](README.ar.md) · [Català](README.ca.md)

[![CI](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml/badge.svg)](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/codewhale-cli?label=crates.io)](https://crates.io/crates/codewhale-cli)
[![npm](https://img.shields.io/npm/v/codewhale?label=npm)](https://www.npmjs.com/package/codewhale)
[![Discord](https://img.shields.io/badge/Discord-join-5865F2?logo=discord&logoColor=white)](https://discord.gg/37gfS3ksug)

## इंस्टॉल करें

```bash
npm install -g codewhale
codewhale
```

पहली बार चलाने पर Codewhale आपको किसी प्रोवाइडर से जुड़ने या ऑफ़लाइन बने रहने में मदद करता है। यह Cargo, Docker, Nix, Scoop, पहले से बने आर्काइव, Android/Termux और CNB मिरर का भी समर्थन करता है। [इंस्टॉलेशन गाइड](docs/INSTALL.md) देखें।

हर शेल में Tab completion के लिए केवल एक कमांड चाहिए — `codewhale completion bash|zsh|fish|powershell|elvish`। [शेल कंप्लीशन](docs/INSTALL.md#8-shell-completions) देखें।

## उपयोग

Codewhale से वैसे ही बात करें जैसे आप अपनी टीम के किसी सदस्य से करेंगे:

```text
Fix the failing tests and explain what changed.
```

या TUI खोले बिना कोई कार्य चलाएँ:

```bash
codewhale exec "fix the failing tests and explain what changed"
```

Codewhale आपकी रिपॉज़िटरी पढ़ सकता है, फ़ाइलें संपादित कर सकता है, कमांड चला सकता है, परिणामों की जाँच कर सकता है और लक्ष्य की ओर काम जारी रख सकता है। उसे कितना एक्सेस देना है, यह आप तय करते हैं।

## Codewhale क्यों

- **अपनी पसंद का मॉडल इस्तेमाल करें।** होस्ट किए गए प्रोवाइडर या Ollama, vLLM अथवा SGLang के माध्यम से लोकल मॉडल जोड़ें। `/model` से प्रोवाइडर और मॉडल बदलें।
- **नियंत्रण अपने पास रखें।** Plan केवल पढ़ने के लिए है। Ask, Auto-Review और Full Access अनुमोदन के व्यवहार को स्पष्ट बनाते हैं। `/undo` पिछला टर्न वापस करता है और `/restore` वर्कस्पेस को पहले के स्नैपशॉट पर लौटाता है।
- **लंबे काम को व्यवस्थित रखें।** सेशन सहेजें, स्थायी `/goal` तय करें, वर्कफ़्लो चलने से पहले उनकी समीक्षा करें और एजेंटों के आंतरिक निर्देशों को अपनी बातचीत में जोड़े बिना उनका समन्वय करें।
- **अपने मौजूदा एजेंट को विस्तृत करें।** MCP सर्वर और स्किल जोड़ें, हुक कॉन्फ़िगर करें और एजेंट की भूमिकाओं को अपने प्रोजेक्ट या निजी सेटिंग में पढ़ने योग्य फ़ाइलों के रूप में रखें।

कमांड और कीबोर्ड शॉर्टकट देखने के लिए TUI में `/help` चलाएँ।

## सुरक्षा

Codewhale आपकी मशीन पर उतने ही एक्सेस के साथ चलता है जितना आप उसे देते हैं। अनुमोदन मोड और रिपॉज़िटरी के नियम एजेंट की गतिविधियों को सीमित करते हैं; समर्थित सिस्टम पर वैकल्पिक OS सैंडबॉक्सिंग अधिक मज़बूत निष्पादन सीमा जोड़ती है। जिन मॉडलों की कीमत ज्ञात नहीं है, उन्हें मुफ़्त बताने के बजाय अज्ञात ही दिखाया जाता है।

नीतियों का सटीक क्रम जानने के लिए [अधिकार क्रम](docs/AUTHORIZATION_ORDER.md) और लोकल सेटिंग के लिए [कॉन्फ़िगरेशन](docs/CONFIGURATION.md) पढ़ें।

## दस्तावेज़

- [प्रोवाइडर और लोकल मॉडल](docs/PROVIDERS.md)
- [एजेंट टीमें](docs/FLEET.md)
- [MCP](docs/MCP.md), [हुक](docs/HOOKS.md) और [कॉन्फ़िगरेशन](docs/CONFIGURATION.md)
- [लोकल वेब क्लाइंट](docs/WEB.md)
- [सभी दस्तावेज़](docs)

## समुदाय से जुड़ें

जब लोग Codewhale का उपयोग करते हैं, असुविधाओं की जानकारी देते हैं और उन्हें ठीक करने में मदद करते हैं, तब यह बेहतर बनता है। यदि कोई प्रोवाइडर उपलब्ध नहीं है, कोई वर्कफ़्लो असहज है या टर्मिनल UI आपके काम में बाधा डालता है, तो [issue खोलें](https://github.com/Hmbown/CodeWhale/issues)। यदि आप इसे बेहतर बनाने का तरीका जानते हैं, तो [pull request खोलें](CONTRIBUTING.md)। पहले योगदान का स्वागत है और स्वीकार किए गए काम का श्रेय योगदानकर्ताओं के पास रहता है।

[Discord](https://discord.gg/37gfS3ksug) से जुड़ें, या WeChat पर Hunter (`hunterbown`) को जोड़कर Whale Brothers समूह में शामिल होने के लिए कहें।

## प्रोजेक्ट का इतिहास

Codewhale की शुरुआत `deepseek-tui` के रूप में हुई थी और यह आज भी उसके कॉन्फ़िगरेशन तथा सेशन के साथ संगतता बनाए रखता है। अब यह किसी प्रोवाइडर पर निर्भर नहीं है, स्वतंत्र रूप से अनुरक्षित है और किसी भी मॉडल प्रोवाइडर से संबद्ध नहीं है।

हर योगदानकर्ता और प्रोजेक्ट को आगे बढ़ाने वाले ओपन सोर्स समुदायों का धन्यवाद। [योगदानकर्ताओं का रिकॉर्ड](docs/CONTRIBUTORS.md) देखें।

## लाइसेंस

[MIT](LICENSE)। अन्य ओपन सोर्स प्रोजेक्ट से लिए और अनुकूलित किए गए हिस्से [थर्ड-पार्टी नोटिस](docs/THIRD_PARTY_NOTICES.md) में दर्ज हैं।
