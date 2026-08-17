<!-- source: README.md sha256:4fc19c5f9596 -->
# Codewhale

وكيل برمجة مفتوح المصدر لطرفيّتك — أحضر نموذجك بنفسك.

بدأ Codewhale تجربة أصلية لـ DeepSeek. ومنذ ذلك الحين نما إلى مشروع تقوده
المجتمع: حزمة برمجة واحدة تناسب مجتمعًا دوليًا متناميًا وتدعم أكبر عدد ممكن من
النماذج والمزوّدين — النماذج المفتوحة أولًا، مستضافة أو محلية، بلا امتياز لأحد
على البقية.

أعطه مزوّدًا ونموذجًا ومهمة. يقرأ شفرتك، يعدّل الملفات، يشغّل الأوامر ويراجع
عمله، ثم يتوقف عندما تكتمل المهمة أو يحتاجك. بدّل النموذج أثناء المهمة بـ
`/model`. اعمل تفاعليًا في TUI، أو شغّل `codewhale exec` في السكربتات وCI.
مكتوب بـ Rust، مرخّص بـ MIT، ويعمل على جهازك.

ما يميّزه عن الحزم الأخرى: **أنت تختار النموذج لكل دور، ولا يلزم أن تتطابق.**
يثبّت الـ fleet مزوّدًا ونموذجًا وطبقة استدلال لكل دور — فيقدر نموذج رخيص سريع
أن يوجّه نموذج استدلال مكلفًا، أو يعمل builder على GLM في المهمة نفسها مع
reviewer على Kimi. اكتب أدوارك ودستور constitution خاصًا بك، فتصبح الحزمة لك لا
لنا.

نبحث دائمًا عن مساهمين وسبل للتحسين. إن غاب نموذج أو مزوّد تستخدمه، أو تعطل
شيء، فإخبارنا من أكثر ما يمكنك فعله نفعًا — انظر [المساهمة](#المساهمة).

[English](README.md) · [简体中文](README.zh-CN.md) · [日本語](README.ja-JP.md) · [Tiếng Việt](README.vi.md) · [Bahasa Indonesia](README.id.md) · [한국어](README.ko-KR.md) · [Español](README.es-419.md) · [Português](README.pt-BR.md) · [Русский](README.ru.md) · [Українська](README.uk.md) · [Français](README.fr.md) · [Deutsch](README.de.md) · [繁體中文](README.zh-TW.md) · [हिन्दी](README.hi.md) · [Türkçe](README.tr.md) · [Italiano](README.it.md) · [Polski](README.pl.md) · [Català](README.ca.md) · [codewhale.net](https://codewhale.net/) · [Docs](docs) · [Changelog](CHANGELOG.md) · [Discord](https://discord.gg/37gfS3ksug)

[![CI](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml/badge.svg)](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/codewhale-cli?label=crates.io)](https://crates.io/crates/codewhale-cli)
[![npm](https://img.shields.io/npm/v/codewhale?label=npm)](https://www.npmjs.com/package/codewhale)
[![Discord](https://img.shields.io/badge/Discord-join%20the%20community-5865F2?logo=discord&logoColor=white)](https://discord.gg/37gfS3ksug)

![Codewhale يعمل في طرفية](assets/screenshot.png)

## التثبيت

```bash
npm install -g codewhale
```

Cargo وDocker وNix وScoop والأرشيفات المبنية مسبقًا وAndroid/Termux ومرآة CNB
لمن لا يصل إلى GitHub مشروحة في
[docs/INSTALL.md](docs/INSTALL.md). قادم من `deepseek-tui`؟ إعدادك وجلساتك
تنتقل معك — انظر [docs/REBRAND.md](docs/REBRAND.md).

## الاستخدام

```bash
codewhale auth set --provider deepseek   # or export ANTHROPIC_API_KEY, etc.
codewhale                                # open the TUI
codewhale exec "fix the failing test"    # headless
codewhale web                            # local browser client on 127.0.0.1
```

في TUI: `/model` يبدّل المزوّد والنموذج معًا، `/fleet` يبني الفريق ويشغّله —
دور واحد في كل مرة، ولكلٍ نموذجه — و`/undo` يرجع آخر دور، و`/restore <N>` يعيد
مساحة العمل إلى لقطة أقدم (`/restore` وحده يسردها). `Tab` يدور Plan / Work /
Operate عندما يكون المحرّر فارغًا — ومع النص يكمّل أوامر الشرطة المائلة وإشارات
`@`. `Shift+Tab` يدور في أي وقت وضع الأذونات Ask / Auto-Review / Full Access.
`!` يشغّل أمر صدفة عبر مسار الموافقة المعتاد.

## ماذا يفعل

- **أي نموذج، أي مزوّد — وأي مزيج منهما.** DeepSeek وClaude وGPT وKimi وGLM وأكثر
  من 30 مزوّدًا، إضافة إلى vLLM أو SGLang أو Ollama الخاص بك بلا مفتاح، كلها عبر
  زمن تشغيل واحد ومجموعة أدوات واحدة. يتتبع الكتالوج تشكيلة كل مزوّد الحيّة —
  يبقى خلفية DeepSeek V4 Pro (الموسومة `DeepSeek-V4-Pro-0813`) قابلة للاستدعاء
  باسم `deepseek-v4-pro`، وGrok 4.6 هو الافتراضي المباشر لـ xAI، وOrcaRouter
  يوجّه عبر `orcarouter/auto`. يسجّل الدور المحفوظ `provider` و`model` وطبقة
  الاستدلال صراحة، فيقدر الـ fleet أن يعبر بائعين عدة في تشغيل واحد، ولا تعتمد
  مسار الدور على أي مزوّد صادف أنه نشط. حدود السياق والأسعار تأتي من المسار
  الحقيقي، والسعر المجهول يظهر مجهولًا لا 0 $.
- **حزمة تكتبها أنت.** الأدوار ملفات تقرأها وتعدّلها — نموذج ووضعية أدوات
  وتعليمات ثابتة لكل دور — تُحفظ في المشروع ليشاركها الفريق، أو بجانب إعداداتك
  الشخصية الأخرى لتتبعك بين المستودعات. يسجّل الدستور constitution كيف تريد
  الوكيل أن يتصرف عبر كل الجلسات، فتتوافق الحزمة مع ممارستك لا ممارستنا.
- **للقراءة فقط حتى تسمح بالمزيد.** وضع Plan لا يغيّر الملفات، والموافقات تقفل
  الأوامر الخطرة. عندما يلفّ صندوق رمل نظام التشغيل أمرًا فعلًا، يصرّح Codewhale:
  Seatbelt على macOS إن توفّر، وbubblewrap اختياري على Linux. يُترجم
  `constitution.json` في المستودع إلى أقفال كتابة لا يتجاوزها حتى Full Access.
- **عمل يمكنك استئنافه.** يسجّل الـ fleet كل خطوة في دفتر للإلحاق فقط، فيلتقط
  `fleet resume` من حيث توقفت.

## التكاملات

- **DeepSeek Harness (dsh) — متصل عبر Codewhale.**
  يربط `codewhale integrations dsh connect` تثبيت `@deepseek-ai/dsh` القائم
  بمسار مزوّد Codewhale وأذوناتك ومساحة عملك، ويضيف `integrations dsh
  install-bundle` حزمة إضافات DSH الاختيارية حتى يحمل `dsh --profile codewhale`
  تلك الهوية وحده. يملك Codewhale الأذونات وسلطة دورة الحياة؛ ويُبقي dsh جلساته
  وملفاته التعريفية وبيانات اعتماده كما هي. انظر
  [docs/INTEGRATIONS_DSH.md](docs/INTEGRATIONS_DSH.md).
- **VS Code.** يفتح هيكل الامتداد الرسمي (`extensions/vscode`) Codewhale في
  طرفية مدمجة ويعرض Agent View للقراءة فقط فوق زمن التشغيل المحلي. هذه معاينة
  تطوير محلية، وليست إصدار سوق بعد.

## اعرف المزيد

- [docs/PROVIDERS.md](docs/PROVIDERS.md) — كل مسار مزوّد: مستضاف وبوابة ومحلي
- [docs/FLEET.md](docs/FLEET.md) — الأساطيل والدفتر والاستئناف
- [docs/WORKFLOW_EXPERIMENTAL_SEARCH.md](docs/WORKFLOW_EXPERIMENTAL_SEARCH.md) — بحث تجريبي مجمّد ومحايد تجاه المزوّد داخل Workflow
- [docs/CONFIGURATION.md](docs/CONFIGURATION.md) — `config.toml` والخطافات
  والدستور constitution
- [docs/AUTHORIZATION_ORDER.md](docs/AUTHORIZATION_ORDER.md) — كيف تتركب
  الأوضاع والخطافات وقواعد الأذونات وأرضيات السلامة وقانون المستودع والموافقات
  والصندوق الرملي
- [docs/HOOKS.md](docs/HOOKS.md) — أحداث الخطاف الأحد عشر في دورة حياة TUI
  وحمولاتها، وأي ثلاثة منها تستطيع توجيه دور (`codewhale exec` وأوامر CLI
  الفرعية لا تطلق خطافات)
- [docs/WEB.md](docs/WEB.md) — عميل المتصفح على العروة المحلية فقط وحدّ
  المصادقة لمرة واحدة

كل ما تبقّى — الأوضاع واختصارات المفاتيح وتفاصيل الصندوق الرملي وMCP وواجهة
زمن التشغيل والعمارة — يعيش في [docs](docs) وعلى
[codewhale.net](https://codewhale.net/).

## المساهمة

المسائل وطلبات السحب وخطوات إعادة الإنتاج والسجلات وطلبات الميزات كلها عمل
مشروع حقيقي، وأول المساهمات مرحّب بها. عندما يتعذّر دمج طلب سحب كما هو، يحصد
المصونون ما يعمل ويبقى المؤلف منسوبًا — في الالتزام وسجل التغييرات و
[docs/CONTRIBUTORS.md](docs/CONTRIBUTORS.md).

- [المسائل المفتوحة](https://github.com/Hmbown/CodeWhale/issues) — مساهمات أولى
  جيدة هنا
- [CONTRIBUTING.md](CONTRIBUTING.md) — إعداد التطوير ومسار طلبات السحب
- [docs/CONTRIBUTORS.md](docs/CONTRIBUTORS.md) — كل من شكّل هذا المشروع
- [اشترِ لي قهوة](https://www.buymeacoffee.com/hmbown)

شكرًا لـ [DeepSeek](https://github.com/deepseek-ai) على النماذج والدعم الذي بدأ
المشروع، ولـ [DataWhale](https://github.com/datawhalechina) 🐋 على الترحيب بنا
في عائلة Whale Brother، ولـ [OpenWarp](https://github.com/zerx-lab/warp) و
[Open Design](https://github.com/nexu-io/open-design) على التعاون في تجربة
الوكيل في الطرفية.

## الرخصة

[MIT](LICENSE). مشروع مجتمعي مستقل، غير مرتبط بأي مزوّد نماذج.

![Codewhale يفتح ثلاثة وكلاء فرعيين scout للقراءة فقط في طرفية](assets/fanout.gif)
