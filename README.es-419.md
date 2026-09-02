<!-- source: README.md sha256:e421b94e9f7e -->
# Codewhale

Codewhale es un agente de programación de código abierto para tu terminal, desarrollado en Rust y mejorado públicamente junto con las personas que lo usan.

![Codewhale ejecutándose en una terminal](assets/screenshot.webp)

[English](README.md) · [简体中文](README.zh-CN.md) · [日本語](README.ja-JP.md) · [Tiếng Việt](README.vi.md) · [Bahasa Indonesia](README.id.md) · [한국어](README.ko-KR.md) · [Português](README.pt-BR.md) · [Русский](README.ru.md) · [Українська](README.uk.md) · [Français](README.fr.md) · [Deutsch](README.de.md) · [繁體中文](README.zh-TW.md) · [हिन्दी](README.hi.md) · [Türkçe](README.tr.md) · [Italiano](README.it.md) · [Polski](README.pl.md) · [العربية](README.ar.md) · [Català](README.ca.md)

[![CI](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml/badge.svg)](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/codewhale-cli?label=crates.io)](https://crates.io/crates/codewhale-cli)
[![npm](https://img.shields.io/npm/v/codewhale?label=npm)](https://www.npmjs.com/package/codewhale)
[![Discord](https://img.shields.io/badge/Discord-join-5865F2?logo=discord&logoColor=white)](https://discord.gg/37gfS3ksug)

## Instalación

```bash
npm install -g codewhale
codewhale
```

La primera vez que se ejecuta, Codewhale te ayuda a conectar un proveedor o a seguir sin conexión. También admite Cargo, Docker, Nix, Scoop, archivos precompilados, Android/Termux y un espejo de CNB. Consulta la [guía de instalación](docs/INSTALL.md).

El completado con Tab se configura con un comando por shell — `codewhale completion bash|zsh|fish|powershell|elvish`. Consulta el [completado de shell](docs/INSTALL.md#8-shell-completions).

## Uso

Habla con Codewhale como hablarías con alguien de tu equipo:

```text
Fix the failing tests and explain what changed.
```

También puedes ejecutar una tarea sin abrir la TUI:

```bash
codewhale exec "fix the failing tests and explain what changed"
```

Codewhale puede leer tu repositorio, editar archivos, ejecutar comandos, revisar los resultados y seguir trabajando para alcanzar un objetivo. Tú decides cuánto acceso darle.

## Por qué Codewhale

- **Usa el modelo que prefieras.** Conecta proveedores alojados o modelos locales mediante Ollama, vLLM o SGLang. Cambia de proveedor y modelo con `/model`.
- **Mantén el control.** Plan es de solo lectura. Ask, Auto-Review y Full Access hacen visible el comportamiento de las aprobaciones. `/undo` revierte el último turno y `/restore` devuelve el espacio de trabajo a una instantánea anterior.
- **Mantén organizado el trabajo de larga duración.** Guarda sesiones, establece un `/goal` duradero, revisa los flujos de trabajo antes de ejecutarlos y coordina agentes sin convertir sus instrucciones internas en parte de tu conversación.
- **Amplía el agente que ya tienes.** Conecta servidores MCP y habilidades, configura hooks y conserva los roles de los agentes como archivos legibles en tu proyecto o configuración personal.

Ejecuta `/help` en la TUI para ver los comandos y atajos de teclado.

## Seguridad

Codewhale se ejecuta en tu equipo con el acceso que le otorgues. Los modos de aprobación y las reglas del repositorio limitan lo que el agente puede hacer; el aislamiento opcional del sistema operativo añade un límite de ejecución más sólido cuando es compatible. Los precios desconocidos de los modelos permanecen como desconocidos en lugar de mostrarse como gratuitos.

Lee el [orden de autorización](docs/AUTHORIZATION_ORDER.md) para conocer la jerarquía exacta de políticas y la [configuración](docs/CONFIGURATION.md) para los ajustes locales.

## Documentación

- [Proveedores y modelos locales](docs/PROVIDERS.md)
- [Equipos de agentes](docs/FLEET.md)
- [MCP](docs/MCP.md), [hooks](docs/HOOKS.md) y [configuración](docs/CONFIGURATION.md)
- [Cliente web local](docs/WEB.md)
- [Toda la documentación](docs)

## Únete a la comunidad

Codewhale mejora cuando las personas lo usan, informan lo que no funciona bien y ayudan a corregirlo. Si falta un proveedor, un flujo de trabajo resulta incómodo o la interfaz de terminal se interpone en tu camino, [abre un issue](https://github.com/Hmbown/CodeWhale/issues). Si sabes cómo mejorarlo, [abre un pull request](CONTRIBUTING.md). Las primeras contribuciones son bienvenidas y quienes contribuyen conservan el crédito por el trabajo que se incorpora.

Únete a [Discord](https://discord.gg/37gfS3ksug), o agrega a Hunter en WeChat (`hunterbown`) y pide entrar al grupo Whale Brothers.

## Historia del proyecto

Codewhale comenzó como `deepseek-tui` y aún conserva la compatibilidad con su configuración y sus sesiones. Ahora es neutral respecto de los proveedores, se mantiene de forma independiente y no está afiliado a ningún proveedor de modelos.

Gracias a cada colaborador y a las comunidades de código abierto que ayudaron a crecer al proyecto. Consulta el [registro de colaboradores](docs/CONTRIBUTORS.md).

## Licencia

[MIT](LICENSE). Las partes adaptadas de otros proyectos de código abierto se registran en los [avisos de terceros](docs/THIRD_PARTY_NOTICES.md).
