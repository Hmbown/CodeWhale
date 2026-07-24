<!-- source: README.md sha256:797a23968c31 -->
# Codewhale

Un agente de programación de código abierto para tu terminal — trae tu propio modelo.

Codewhale empezó como una experiencia nativa para DeepSeek. Desde entonces se ha
convertido en un proyecto impulsado por la comunidad: un harness de programación
que se adapta a una comunidad internacional en crecimiento y admite tantos
modelos y proveedores como sea posible — los modelos abiertos primero, alojados
o locales, sin privilegiar a ninguno.

Le das un proveedor, un modelo y una tarea. Lee tu código, edita archivos,
ejecuta comandos y verifica su propio trabajo, y se detiene cuando la tarea
queda lista o te necesita. Cambia de modelo a mitad de tarea con `/model`.
Trabaja de forma interactiva en la TUI, o ejecuta `codewhale exec` en scripts y
CI. Está escrito en Rust, con licencia MIT, y corre en tu máquina.

Siempre estamos buscando personas que contribuyan y formas de mejorar. Si falta
un modelo o proveedor que usas, o algo se rompe, contárnoslo es una de las cosas
más útiles que puedes hacer — mira [Contribuir](#contribuir).

[English](README.md) · [简体中文](README.zh-CN.md) · [日本語](README.ja-JP.md) · [Tiếng Việt](README.vi.md) · [한국어](README.ko-KR.md) · [Português](README.pt-BR.md) · [codewhale.net](https://codewhale.net/) · [Docs](docs) · [Changelog](CHANGELOG.md)

[![CI](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml/badge.svg)](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/codewhale-cli?label=crates.io)](https://crates.io/crates/codewhale-cli)
[![npm](https://img.shields.io/npm/v/codewhale?label=npm)](https://www.npmjs.com/package/codewhale)

![Codewhale ejecutándose en una terminal](assets/screenshot.png)

## Instalación

```bash
npm install -g codewhale
```

Cargo, Docker, Nix, Scoop, archivos precompilados, Android/Termux y un espejo
en CNB para quienes no pueden acceder a GitHub están cubiertos en
[docs/INSTALL.md](docs/INSTALL.md). ¿Vienes de `deepseek-tui`? Tu configuración
y tus sesiones se conservan — mira [docs/REBRAND.md](docs/REBRAND.md).

## Uso

```bash
codewhale auth set --provider deepseek   # or export ANTHROPIC_API_KEY, etc.
codewhale                                # open the TUI
codewhale exec "fix the failing test"    # headless
codewhale web                            # local browser client on 127.0.0.1
```

En la TUI: `/model` cambia proveedor y modelo juntos, `/fleet` ejecuta un
equipo de workers y `/restore` deshace un turno. Cuando el compositor está
inactivo, `Tab` cicla entre Plan / Act / Operate y `Shift+Tab` cicla la postura
de permiso Ask / Auto-Review / Full Access. `!` ejecuta un comando de shell por
la ruta normal de aprobación.

## Qué hace

- **Cualquier modelo, cualquier proveedor.** DeepSeek, Claude, GPT, Kimi, GLM y
  más de 30 proveedores, además de tu propio vLLM, SGLang u Ollama sin key —
  todo a través de un solo runtime y un solo conjunto de herramientas. Los
  presupuestos de contexto y los precios vienen de la ruta real, y un precio
  desconocido se muestra como desconocido en lugar de $0.
- **Solo lectura hasta que permitas más.** El modo Plan no cambia archivos, y
  las aprobaciones controlan los comandos riesgosos. Cuando un sandbox del
  sistema operativo realmente envuelve un comando, Codewhale lo indica: Seatbelt
  en macOS cuando está disponible, bubblewrap opcional en Linux. El
  `constitution.json` de un repo se compila en bloqueos de escritura que ni
  siquiera Full Access puede saltarse.
- **Trabajo que puedes retomar.** Un fleet registra cada paso en un libro mayor
  de solo agregado, así que `fleet resume` retoma donde te detuviste.

## Para saber más

- [docs/PROVIDERS.md](docs/PROVIDERS.md) — cada ruta de proveedor: alojada,
  gateway y local
- [docs/FLEET.md](docs/FLEET.md) — fleets, el libro mayor y resume
- [docs/CONFIGURATION.md](docs/CONFIGURATION.md) — `config.toml`, hooks y la
  constitution
- [docs/WEB.md](docs/WEB.md) — cliente de navegador integrado solo en loopback
  y su límite de autenticación de un solo uso

Todo lo demás — modos, atajos de teclado, detalles del sandbox, MCP, la API
del runtime, arquitectura — está en [docs](docs) y en
[codewhale.net](https://codewhale.net/).

## Contribuir

Issues, PRs, pasos de reproducción, logs y solicitudes de features son trabajo
real del proyecto, y las primeras contribuciones son bienvenidas. Cuando un PR
no se puede fusionar tal cual, los mantenedores rescatan lo que funciona y el
autor conserva su crédito — en el commit, en el changelog y en
[docs/CONTRIBUTORS.md](docs/CONTRIBUTORS.md).

- [Issues abiertos](https://github.com/Hmbown/CodeWhale/issues) — las buenas
  primeras contribuciones viven aquí
- [CONTRIBUTING.md](CONTRIBUTING.md) — setup de desarrollo y flujo de PRs
- [docs/CONTRIBUTORS.md](docs/CONTRIBUTORS.md) — todas las personas que le han
  dado forma a esto
- [Invítame un café](https://www.buymeacoffee.com/hmbown)

Gracias a [DeepSeek](https://github.com/deepseek-ai) por los modelos y el apoyo
que dieron inicio al proyecto, a [DataWhale](https://github.com/datawhalechina)
🐋 por recibirnos en la familia Whale Brother, y a
[OpenWarp](https://github.com/zerx-lab/warp) y
[Open Design](https://github.com/nexu-io/open-design) por colaborar en la
experiencia de agente en terminal.

## Licencia

[MIT](LICENSE). Proyecto comunitario independiente; sin afiliación con ningún
proveedor de modelos.

[![Star History Chart](https://api.star-history.com/chart?repos=Hmbown/CodeWhale&type=date&legend=top-left)](https://www.star-history.com/?repos=Hmbown%2FCodeWhale&type=date)
