"""
DeepSeek IDE - Extension Layer Entry Point
Starts ProcessManager + PlaywrightWrapper in parallel.
This process is launched as a Tauri sidecar.
"""

import asyncio
import sys
from loguru import logger
from process_manager import ProcessManager
from playwright_wrapper import PlaywrightWrapper
from api.server import ExtensionServer

# Pretty log format
logger.remove()
logger.add(
    sys.stderr,
    format="<green>{time:HH:mm:ss}</green> | <level>{level: <8}</level> | {message}",
    colorize=True,
)
logger.add(
    "logs/extension.log",
    rotation="10 MB",
    retention="7 days",
    encoding="utf-8",
)


async def main():
    pm = ProcessManager()
    pw = PlaywrightWrapper(pm)
    server = ExtensionServer(pm, pw)

    logger.info("=== DeepSeek IDE Extension Layer Starting ===")

    # Start TUI process in background
    tui_task = asyncio.create_task(pm.start())

    # Give TUI a moment to bind its port
    await asyncio.sleep(2.0)

    await server.start()

    # Start Playwright layer (waits for TUI to be ready)
    await pw.start()

    logger.info("All components ready. Listening...")

    try:
        # Keep running until TUI exits or Ctrl+C
        await tui_task
    except asyncio.CancelledError:
        pass
    except KeyboardInterrupt:
        logger.info("Shutdown signal received.")
    finally:
        logger.info("Shutting down...")
        await server.stop()
        await pw.stop()
        await pm.shutdown()
        logger.info("=== Extension Layer Stopped ===")


if __name__ == "__main__":
    asyncio.run(main())

