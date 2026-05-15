import os
from typing import Any

from aiohttp import web
from loguru import logger


EXTENSION_HOST = os.getenv("DEEPSEEK_EXTENSION_HOST", "127.0.0.1")
EXTENSION_PORT = int(os.getenv("DEEPSEEK_EXTENSION_PORT", "3000"))


class ExtensionServer:
    def __init__(self, process_manager, playwright_wrapper):
        self._pm = process_manager
        self._pw = playwright_wrapper
        self._runner: web.AppRunner | None = None
        self._site: web.TCPSite | None = None
        self._app = web.Application()
        self._app.add_routes(
            [
                web.get("/health", self.health),
                web.get("/v1/status", self.status),
                web.get("/v1/capabilities", self.capabilities),
                web.get("/v1/browser/state", self.browser_state),
                web.get("/v1/browser/summary", self.browser_summary),
                web.post("/v1/chat", self.chat),
                web.post("/v1/chat/stream", self.chat_stream),
                web.post("/v1/browser/dom", self.browser_dom),
                web.post("/v1/browser/element", self.browser_element),
                web.post("/v1/browser/navigate", self.browser_navigate),
                web.post("/v1/browser/evaluate", self.browser_evaluate),
                web.post("/v1/browser/screenshot", self.browser_screenshot),
                web.post("/v1/browser/ui", self.browser_ui),
                web.post("/v1/browser/reload-console", self.browser_reload_console),
                web.post("/v1/browser/mcp-task", self.browser_mcp_task),
                web.post("/control/start", self.start_runtime),
                web.post("/control/restart", self.restart),
                web.post("/control/stop", self.stop_runtime),
                web.get("/ui/host", self.ui_host),
            ]
        )

    async def start(self):
        self._runner = web.AppRunner(self._app)
        await self._runner.setup()
        self._site = web.TCPSite(self._runner, EXTENSION_HOST, EXTENSION_PORT)
        await self._site.start()
        logger.info(f"Extension API listening on http://{EXTENSION_HOST}:{EXTENSION_PORT}")

    async def stop(self):
        if self._runner:
            await self._runner.cleanup()
            self._runner = None
            self._site = None

    async def health(self, _request: web.Request):
        return web.json_response({"status": "ok", "service": "deepseek-extension-api"})

    async def status(self, _request: web.Request):
        return web.json_response(await self._pw.get_status())

    async def capabilities(self, _request: web.Request):
        return web.json_response(await self._pw.get_capabilities())

    async def browser_state(self, _request: web.Request):
        return web.json_response(await self._pw.get_browser_state())

    async def browser_summary(self, _request: web.Request):
        return web.json_response(await self._pw.get_page_summary())

    async def chat(self, request: web.Request):
        payload = await request.json()
        message = (payload.get("message") or "").strip()
        if not message:
            return web.json_response({"error": "message is required"}, status=400)

        content = await self._pw.send_message(message)
        return web.json_response({"content": content})

    async def chat_stream(self, request: web.Request):
        payload = await request.json()
        message = (payload.get("message") or "").strip()
        if not message:
            return web.Response(status=400, text="message is required")

        content = await self._pw.send_message(message)
        return web.Response(text=content, content_type="text/plain")

    async def browser_navigate(self, request: web.Request):
        payload = await self._read_json(request)
        url = (payload.get("url") or "").strip()
        if not url:
            return web.json_response({"error": "url is required"}, status=400)
        return web.json_response(await self._pw.navigate(url))

    async def browser_dom(self, request: web.Request):
        payload = await self._read_json(request)
        selector = (payload.get("selector") or "").strip()
        if not selector:
            return web.json_response({"error": "selector is required"}, status=400)
        return web.json_response(await self._pw.inspect_dom(selector))

    async def browser_element(self, request: web.Request):
        payload = await self._read_json(request)
        selector = (payload.get("selector") or "").strip()
        action = (payload.get("action") or "").strip()
        if not selector or not action:
            return web.json_response({"error": "selector and action are required"}, status=400)
        return web.json_response(
            await self._pw.operate_element(selector, action, payload.get("value"))
        )

    async def browser_evaluate(self, request: web.Request):
        payload = await self._read_json(request)
        expression = (payload.get("expression") or "").strip()
        if not expression:
            return web.json_response({"error": "expression is required"}, status=400)
        return web.json_response({"result": await self._pw.evaluate(expression)})

    async def browser_screenshot(self, request: web.Request):
        payload = await self._read_json(request)
        return web.json_response(await self._pw.screenshot(payload.get("path")))

    async def browser_ui(self, request: web.Request):
        payload = await self._read_json(request)
        return web.json_response(
            await self._pw.update_ui(
                viewport_width=payload.get("viewport_width"),
                viewport_height=payload.get("viewport_height"),
                zoom=payload.get("zoom"),
                css=payload.get("css"),
                js=payload.get("js"),
                reset_injections=bool(payload.get("reset_injections", False)),
            )
        )

    async def browser_reload_console(self, _request: web.Request):
        return web.json_response(await self._pw.reload_console())

    async def browser_mcp_task(self, request: web.Request):
        payload = await self._read_json(request)
        prompt = (payload.get("prompt") or "").strip()
        if not prompt:
            return web.json_response({"error": "prompt is required"}, status=400)
        return web.json_response(await self._pw.run_mcp_browser_task(prompt))

    async def start_runtime(self, _request: web.Request):
        await self._pm.ensure_started()
        return web.json_response({"ok": True, "desired_running": self._pm.desired_running})

    async def restart(self, _request: web.Request):
        await self._pm.restart()
        return web.json_response({"ok": True})

    async def stop_runtime(self, _request: web.Request):
        await self._pm.stop()
        return web.json_response({"ok": True})

    async def _read_json(self, request: web.Request) -> dict[str, Any]:
        if request.can_read_body:
            try:
                return await request.json()
            except Exception:
                return {}
        return {}

    async def ui_host(self, _request: web.Request):
        return web.Response(
            text="""
<!DOCTYPE html>
<html lang=\"zh-CN\">
<head>
  <meta charset=\"utf-8\" />
  <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\" />
  <title>DeepSeek Control Console</title>
  <style>
    html, body {
      margin: 0;
      width: 100%;
      height: 100%;
      background: radial-gradient(circle at top, #182334 0%, #0b1017 58%, #06080d 100%);
      overflow: hidden;
    }
  </style>
</head>
<body>
</body>
</html>
            """.strip(),
            content_type="text/html",
        )
