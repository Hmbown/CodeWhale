<!-- source: README.md sha256:e421b94e9f7e -->
# Codewhale

Codewhale, terminaliniz için Rust ile geliştirilmiş ve kullanıcılarıyla birlikte açık biçimde iyileştirilen açık kaynaklı bir kodlama ajanıdır.

![Terminalde çalışan Codewhale](assets/screenshot.webp)

[English](README.md) · [简体中文](README.zh-CN.md) · [日本語](README.ja-JP.md) · [Tiếng Việt](README.vi.md) · [Bahasa Indonesia](README.id.md) · [한국어](README.ko-KR.md) · [Español](README.es-419.md) · [Português](README.pt-BR.md) · [Русский](README.ru.md) · [Українська](README.uk.md) · [Français](README.fr.md) · [Deutsch](README.de.md) · [繁體中文](README.zh-TW.md) · [हिन्दी](README.hi.md) · [Italiano](README.it.md) · [Polski](README.pl.md) · [العربية](README.ar.md) · [Català](README.ca.md)

[![CI](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml/badge.svg)](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/codewhale-cli?label=crates.io)](https://crates.io/crates/codewhale-cli)
[![npm](https://img.shields.io/npm/v/codewhale?label=npm)](https://www.npmjs.com/package/codewhale)
[![Discord](https://img.shields.io/badge/Discord-join-5865F2?logo=discord&logoColor=white)](https://discord.gg/37gfS3ksug)

## Kurulum

```bash
npm install -g codewhale
codewhale
```

Codewhale ilk çalıştırmada bir sağlayıcıya bağlanmanıza veya çevrimdışı kalmanıza yardımcı olur. Cargo, Docker, Nix, Scoop, önceden derlenmiş arşivler, Android/Termux ve CNB aynasını da destekler. [Kurulum kılavuzuna](docs/INSTALL.md) bakın.

Her kabukta Tab tamamlama tek bir komutla etkinleştirilir — `codewhale completion bash|zsh|fish|powershell|elvish`. [Kabuk tamamlamalarına](docs/INSTALL.md#8-shell-completions) bakın.

## Kullanım

Codewhale ile ekip arkadaşınızla konuşur gibi konuşun:

```text
Fix the failing tests and explain what changed.
```

TUI’yi açmadan da bir görev çalıştırabilirsiniz:

```bash
codewhale exec "fix the failing tests and explain what changed"
```

Codewhale deponuzu okuyabilir, dosyaları düzenleyebilir, komutları çalıştırabilir, sonuçları inceleyebilir ve bir hedefe doğru çalışmayı sürdürebilir. Ne kadar erişime sahip olacağına siz karar verirsiniz.

## Neden Codewhale

- **İstediğiniz modeli kullanın.** Barındırılan sağlayıcılara veya Ollama, vLLM ya da SGLang üzerinden yerel modellere bağlanın. Sağlayıcı ve modeli `/model` ile değiştirin.
- **Kontrolü elinizde tutun.** Plan salt okunurdur. Ask, Auto-Review ve Full Access, onay davranışını görünür kılar. `/undo` son turu geri alır, `/restore` ise çalışma alanını önceki bir anlık görüntüye döndürür.
- **Uzun süren işleri düzenli tutun.** Oturumları kaydedin, kalıcı bir `/goal` belirleyin, iş akışlarını çalışmadan önce gözden geçirin ve ajanların iç talimatlarını konuşmanıza taşımadan onları koordine edin.
- **Elinizdeki ajanı genişletin.** MCP sunucularını ve becerileri bağlayın, hook’ları yapılandırın ve ajan rollerini projenizde veya kişisel ayarlarınızda okunabilir dosyalar olarak saklayın.

Komutları ve klavye kısayollarını görmek için TUI’de `/help` komutunu çalıştırın.

## Güvenlik

Codewhale, verdiğiniz erişimle kendi makinenizde çalışır. Onay modları ve depo kuralları ajanın yapabileceklerini sınırlar; desteklenen ortamlarda isteğe bağlı işletim sistemi sandbox’ı daha güçlü bir yürütme sınırı ekler. Bilinmeyen model fiyatları ücretsiz olarak bildirilmek yerine bilinmeyen olarak kalır.

Politikaların kesin sıralaması için [yetkilendirme sırasını](docs/AUTHORIZATION_ORDER.md), yerel ayarlar için [yapılandırmayı](docs/CONFIGURATION.md) okuyun.

## Belgeler

- [Sağlayıcılar ve yerel modeller](docs/PROVIDERS.md)
- [Ajan ekipleri](docs/FLEET.md)
- [MCP](docs/MCP.md), [hook’lar](docs/HOOKS.md) ve [yapılandırma](docs/CONFIGURATION.md)
- [Yerel web istemcisi](docs/WEB.md)
- [Tüm belgeler](docs)

## Topluluğa katılın

İnsanlar Codewhale’i kullandıkça, yanlış gelen noktaları bildirdikçe ve düzeltmeye yardımcı oldukça Codewhale daha iyi olur. Bir sağlayıcı eksikse, bir iş akışı kullanışsızsa veya terminal arayüzü işinizi zorlaştırıyorsa [bir issue açın](https://github.com/Hmbown/CodeWhale/issues). Nasıl iyileştirileceğini biliyorsanız [bir pull request açın](CONTRIBUTING.md). İlk katkılar memnuniyetle karşılanır ve katkıda bulunanların projeye alınan çalışmaları üzerindeki emeği kayda geçer.

[Discord’a](https://discord.gg/37gfS3ksug) katılın veya WeChat’te Hunter’ı (`hunterbown`) ekleyip Whale Brothers grubuna katılmak istediğinizi belirtin.

## Proje geçmişi

Codewhale, `deepseek-tui` olarak başladı ve onun yapılandırması ile oturumlarıyla uyumluluğunu hâlâ koruyor. Artık sağlayıcılardan bağımsızdır, bağımsız olarak sürdürülür ve herhangi bir model sağlayıcısıyla bağlantılı değildir.

Projeyi büyütmeye yardımcı olan tüm katkıcılara ve açık kaynak topluluklarına teşekkürler. [Katkıcı kaydına](docs/CONTRIBUTORS.md) bakın.

## Lisans

[MIT](LICENSE). Diğer açık kaynak projelerinden uyarlanan bölümler [üçüncü taraf bildirimlerinde](docs/THIRD_PARTY_NOTICES.md) kayıtlıdır.
