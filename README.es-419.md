<!-- source: README.md sha256:ddc9cd7b5aea -->
# Codewhale

Un agente de código para tu terminal. Funciona con cualquier modelo; los
modelos abiertos primero.

Le das un proveedor, un modelo y una tarea. Lee código, edita archivos, ejecuta
comandos, verifica los resultados y sigue avanzando hasta que la tarea queda
lista o te necesita. TUI para el trabajo interactivo, `codewhale exec` para
scripts y CI. Rust, MIT, corre completamente en tu máquina.

Empezó como `deepseek-tui`. La comunidad que se formó a su alrededor necesitaba
más proveedores, así que ahora DeepSeek, Claude, GPT, Kimi, GLM y más de 30
otros corren sobre el mismo runtime y las mismas herramientas.

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
```

En la TUI: `/model` cambia proveedor y modelo juntos, `/fleet` ejecuta un
equipo de workers, `/restore` deshace un turno, `Tab` cicla entre
Plan / Act / Operate, `Shift+Tab` cicla la postura de aprobación
Ask / Auto-Review / Full Access, y `!` ejecuta un comando de shell por la ruta
normal de aprobación.

## Qué hace

- Resuelve tu elección de proveedor + modelo a una ruta concreta: endpoint,
  wire protocol, límite de contexto, precio. Los presupuestos de contexto y el
  costo que se muestra vienen de la ruta real; un precio desconocido se muestra
  como desconocido, no como $0.
  ([docs/PROVIDERS.md](docs/PROVIDERS.md))
- Habla con proveedores que alojan modelos abiertos (`deepseek`, `openrouter`,
  `moonshot`, `zai`, `minimax`, `nvidia-nim`, …), con tu propio `vllm` /
  `sglang` / `ollama` sin clave, y con Anthropic de forma nativa sobre la
  Messages API, con thinking y caché de prompts.
- Ejecuta múltiples workers de forma durable: Fleet registra el trabajo en un
  ledger append-only, así que las ejecuciones sobreviven reinicios y
  `fleet resume` retoma donde quedaron las cosas. Workflow planifica trabajos
  más grandes en carriles reanudables y verificables.
  ([docs/FLEET.md](docs/FLEET.md))
- Regula el riesgo con código, no con corazonadas: tres modos (Plan es de solo
  lectura), una postura de aprobación separada, sandbox a nivel del sistema
  operativo (Seatbelt, Landlock + seccomp, bwrap), hooks que pueden
  permitir/denegar/preguntar por cada llamada a herramienta, y snapshots en un
  git paralelo para que `/restore` nunca toque tu historial real.
- Permite que un repo declare su propia ley: los invariantes de
  `.codewhale/constitution.json` se compilan en bloqueos de escritura que ni
  siquiera Full Access puede saltarse.
  ([docs/CONFIGURATION.md](docs/CONFIGURATION.md))
- Habla MCP en ambas direcciones, carga skills reutilizables, expone APIs de
  runtime HTTP/SSE y ACP, y respalda una
  [GUI para VS Code](https://github.com/HengQuWorld/CodeWhale-VSCode) de la
  comunidad.
- La TUI muestra el trabajo como recibos que puedes inspeccionar, mantiene en
  movimiento una sola fila en vivo, tiene un inspector de contexto real, 12
  temas, modos de movimiento reducido y ASCII seguro, y está disponible en
  English, 简体中文, 日本語, Tiếng Việt, Español, Português, 한국어 y 繁體中文
  parcial.

Todo lo demás — configuración, atajos de teclado, detalles del sandbox,
arquitectura — está en [docs](docs) y en [codewhale.net](https://codewhale.net/).

## Contribuir

Todo feedback es un regalo. Issues, PRs, pasos de reproducción, logs,
solicitudes de features y primeras contribuciones: todo eso es trabajo real del
proyecto aquí. Cuando un PR no se puede fusionar tal cual, los mantenedores
rescatan lo que funciona y el autor conserva su crédito — en el commit, en el
changelog y en [docs/CONTRIBUTORS.md](docs/CONTRIBUTORS.md). Si falta un modelo
o proveedor que usas, o algo se rompe en tu máquina, decírnoslo es lo más útil
que puedes hacer.

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
