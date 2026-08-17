<!-- source: README.md sha256:4fc19c5f9596 -->
# Codewhale

Terminaliniz için açık kaynak bir kodlama ajanı — modeli siz getirin.

Codewhale, DeepSeek için yerel bir deneyim olarak başladı. O zamandan beri
topluluk güdümlü bir projeye dönüştü: büyüyen uluslararası bir topluluğa uyan
ve olabildiğince çok modeli ve sağlayıcıyı destekleyen tek bir kodlama
harness'i — önce açık modeller, barındırılan veya yerel, hiçbiri diğerinden
ayrıcalıklı değil.

Ona bir sağlayıcı, bir model ve bir görev verin. Kodunuzu okur, dosyaları
düzenler, komutları çalıştırır ve kendi işini denetler; iş bitince veya size
ihtiyaç duyunca durur. Görev sırasında `/model` ile model değiştirin. TUI'de
etkileşimli çalışın veya betiklerde ve CI'da `codewhale exec` çalıştırın. Rust
ile yazılmıştır, MIT lisanslıdır ve sizin makinenizde çalışır.

Diğer harness'lerden farkı şu: **her rol için modeli siz seçersiniz ve
birbirleriyle aynı olmak zorunda değiller.** Bir fleet her rol için bir
sağlayıcı, bir model ve bir akıl yürütme katmanı sabitler — böylece ucuz ve
hızlı bir model pahalı bir akıl yürütme modelini yönetebilir veya bir GLM
builder, bir Kimi reviewer ile aynı işte çalışabilir. Kendi rollerinizi, kendi
constitution'ınızı yazın; harness bizim değil sizin olur.

Her zaman katkıda bulunanlar ve iyileştirme yolları arıyoruz. Kullandığınız bir
model veya sağlayıcı eksikse ya da bir şey bozulursa, bize söylemek
yapabileceğiniz en yararlı şeylerden biridir — bkz.
[Katkıda bulunma](#katkıda-bulunma).

[English](README.md) · [简体中文](README.zh-CN.md) · [日本語](README.ja-JP.md) · [Tiếng Việt](README.vi.md) · [Bahasa Indonesia](README.id.md) · [한국어](README.ko-KR.md) · [Español](README.es-419.md) · [Português](README.pt-BR.md) · [Русский](README.ru.md) · [Українська](README.uk.md) · [Français](README.fr.md) · [Deutsch](README.de.md) · [繁體中文](README.zh-TW.md) · [हिन्दी](README.hi.md) · [Italiano](README.it.md) · [Polski](README.pl.md) · [العربية](README.ar.md) · [Català](README.ca.md) · [codewhale.net](https://codewhale.net/) · [Docs](docs) · [Changelog](CHANGELOG.md) · [Discord](https://discord.gg/37gfS3ksug)

[![CI](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml/badge.svg)](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/codewhale-cli?label=crates.io)](https://crates.io/crates/codewhale-cli)
[![npm](https://img.shields.io/npm/v/codewhale?label=npm)](https://www.npmjs.com/package/codewhale)
[![Discord](https://img.shields.io/badge/Discord-join%20the%20community-5865F2?logo=discord&logoColor=white)](https://discord.gg/37gfS3ksug)

![Terminalde çalışan Codewhale](assets/screenshot.png)

## Kurulum

```bash
npm install -g codewhale
```

Cargo, Docker, Nix, Scoop, önceden derlenmiş arşivler, Android/Termux ve
GitHub'a ulaşamayanlar için bir CNB aynası
[docs/INSTALL.md](docs/INSTALL.md) içinde. `deepseek-tui`'den mi geliyorsunuz?
Yapılandırmanız ve oturumlarınız taşınır — bkz.
[docs/REBRAND.md](docs/REBRAND.md).

## Kullanım

```bash
codewhale auth set --provider deepseek   # or export ANTHROPIC_API_KEY, etc.
codewhale                                # open the TUI
codewhale exec "fix the failing test"    # headless
codewhale web                            # local browser client on 127.0.0.1
```

TUI'de: `/model` sağlayıcıyı ve modeli birlikte değiştirir, `/fleet` ekibi
kurar ve çalıştırır — her seferinde bir rol, her biri kendi modeliyle —,
`/undo` son turu geri alır ve `/restore <N>` çalışma alanını önceki bir
anlık görüntüye döndürür (yalın `/restore` bunları listeler). Besteci boşken
`Tab` Plan / Work / Operate arasında döner — içinde metin varken `Tab` slash
komutlarını ve `@` bahsetmelerini tamamlar. `Shift+Tab` her an Ask /
Auto-Review / Full Access izin duruşunu döngüler. `!` bir kabuk komutunu
normal onay yolundan çalıştırır.

## Ne yapar

- **Herhangi bir model, herhangi bir sağlayıcı — ve herhangi bir karışım.**
  DeepSeek, Claude, GPT, Kimi, GLM ve 30'dan fazla sağlayıcı, ayrıca anahtarsız
  kendi vLLM, SGLang veya Ollama'nız, hepsi tek bir çalışma zamanı ve tek bir
  araç setinden. Katalog her sağlayıcının canlı kadrosunu izler — DeepSeek'in
  V4 Pro arka ucu (`DeepSeek-V4-Pro-0813` etiketli) hâlâ `deepseek-v4-pro`
  olarak çağrılabilir, Grok 4.6 doğrudan xAI varsayılanıdır ve OrcaRouter
  `orcarouter/auto` üzerinden yönlendirir. Kaydedilmiş bir rol `provider`,
  `model` ve akıl yürütme katmanını açıkça kaydeder; böylece bir fleet tek bir
  çalıştırmada birden çok satıcıya yayılabilir ve bir rolün rotası o anda
  hangi sağlayıcının etkin olduğuna bağlı olmaz. Bağlam sınırları ve fiyatlar
  gerçek rotadan gelir; bilinmeyen bir fiyat $0 değil, bilinmiyor olarak
  görünür.
- **Sizin yazdığınız bir harness.** Roller okuyup düzenleyebileceğiniz
  dosyalardır — rol başına bir model, bir araç duruşu ve kalıcı yönergeler —
  ekibin paylaşması için projede veya sizi depolar arasında izlemesi için
  diğer kişisel ayarlarınızın yanında tutulur. Bir constitution, ajanın her
  oturumda nasıl davranmasını istediğinizi kaydeder; böylece harness bizim
  değil sizin pratiğinize uyar.
- **Daha fazlasına izin verene kadar salt okunur.** Plan kipi dosyaları
  değiştiremez ve onaylar riskli komutları kapılar. Bir işletim sistemi
  sanal alanı bir komutu gerçekten sardığında Codewhale bunu söyler: macOS'ta
  varsa Seatbelt, Linux'ta isteğe bağlı bubblewrap. Bir deponun
  `constitution.json` dosyası, Full Access'in bile atlayamayacağı yazma
  kilitlerine derlenir.
- **Kaldığınız yerden sürebileceğiniz iş.** Bir fleet her adımı yalnızca
  eklenen bir deftere yazar, böylece `fleet resume` kaldığınız yerden devam
  eder.

## Entegrasyonlar

- **DeepSeek Harness (dsh) — Codewhale üzerinden bağlı.**
  `codewhale integrations dsh connect` mevcut bir `@deepseek-ai/dsh`
  kurulumunu Codewhale sağlayıcı rotanıza, izinlerinize ve çalışma alanınıza
  bağlar; `integrations dsh install-bundle` isteğe bağlı DSH eklenti paketini
  ekler ki `dsh --profile codewhale` bu kimliği kendi başına taşısın.
  İzinler ve yaşam döngüsü yetkisi Codewhale'dedir; dsh kendi oturumlarını,
  profillerini ve kimlik bilgilerini olduğu gibi bırakır. Bkz.
  [docs/INTEGRATIONS_DSH.md](docs/INTEGRATIONS_DSH.md).
- **VS Code.** Resmi eklenti iskelesi (`extensions/vscode`) Codewhale'i
  tümleşik bir terminalde açar ve yerel çalışma zamanı üzerinde salt okunur
  bir Agent View sunar. Bu yerel geliştirme önizlemesidir, henüz bir
  marketplace sürümü değildir.

## Daha fazla öğrenin

- [docs/PROVIDERS.md](docs/PROVIDERS.md) — her sağlayıcı rotası: barındırılan,
  ağ geçidi ve yerel
- [docs/FLEET.md](docs/FLEET.md) — fleet'ler, defter ve devam ettirme
- [docs/WORKFLOW_EXPERIMENTAL_SEARCH.md](docs/WORKFLOW_EXPERIMENTAL_SEARCH.md) — Workflow içinde dondurulmuş, sağlayıcıdan bağımsız deneysel arama
- [docs/CONFIGURATION.md](docs/CONFIGURATION.md) — `config.toml`, hook'lar ve
  constitution
- [docs/AUTHORIZATION_ORDER.md](docs/AUTHORIZATION_ORDER.md) — kipler,
  hook'lar, izin kuralları, güvenlik tabanları, depo yasası, onaylar ve
  sanal alanın nasıl birleştiği
- [docs/HOOKS.md](docs/HOOKS.md) — on bir TUI yaşam döngüsü hook olayı,
  yükleri ve bir turu yönlendirebilen üçü (`codewhale exec` ve CLI alt
  komutları hook tetiklemez)
- [docs/WEB.md](docs/WEB.md) — yalnızca döngü geri dönüşü tarayıcı istemcisi
  ve tek kullanımlık kimlik doğrulama sınırı

Geri kalan her şey — kipler, tuş bağları, sanal alan ayrıntıları, MCP,
çalışma zamanı API'si ve mimari — [docs](docs) ve
[codewhale.net](https://codewhale.net/) üzerinde yaşar.

## Katkıda bulunma

Issue'lar, PR'lar, yeniden üretim adımları, günlükler ve özellik istekleri
gerçek proje işidir ve ilk katkılar hoş karşılanır. Bir PR olduğu gibi
birleştirilemediğinde bakımcılar işe yarayanı alır ve yazarın kredisi kalır
— commit'te, changelog'da ve
[docs/CONTRIBUTORS.md](docs/CONTRIBUTORS.md) içinde.

- [Açık issue'lar](https://github.com/Hmbown/CodeWhale/issues) — iyi ilk
  katkılar burada
- [CONTRIBUTING.md](CONTRIBUTING.md) — geliştirme kurulumu ve PR akışı
- [docs/CONTRIBUTORS.md](docs/CONTRIBUTORS.md) — bunu şekillendiren herkes
- [Bana bir kahve ısmarla](https://www.buymeacoffee.com/hmbown)

Projeyi başlatan modeller ve destek için [DeepSeek](https://github.com/deepseek-ai)'e,
bizi Whale Brother ailesine kabul ettiği için
[DataWhale](https://github.com/datawhalechina) 🐋'e ve terminal ajanı
deneyiminde iş birliği için [OpenWarp](https://github.com/zerx-lab/warp) ile
[Open Design](https://github.com/nexu-io/open-design)'a teşekkürler.

## Lisans

[MIT](LICENSE). Bağımsız bir topluluk projesi; hiçbir model sağlayıcısıyla
bağlı değildir.

![Codewhale bir terminalde üç salt okunur scout alt ajanını açıyor](assets/fanout.gif)
