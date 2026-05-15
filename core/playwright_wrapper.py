import os
import asyncio
import json
import pathlib
from typing import Any
import aiohttp
from loguru import logger
from process_manager import ProcessManager
from playwright.async_api import Browser, BrowserContext, Page, Playwright, async_playwright




RUNTIME_PORT  = int(os.getenv("DEEPSEEK_RUNTIME_PORT", "7878"))
RUNTIME_TOKEN = os.getenv("DEEPSEEK_RUNTIME_TOKEN", "")
RUNTIME_BASE  = f"http://127.0.0.1:{RUNTIME_PORT}"
EXTENSION_PORT = int(os.getenv("DEEPSEEK_EXTENSION_PORT", "3000"))
EXTENSION_BASE = f"http://127.0.0.1:{EXTENSION_PORT}"
CONTROL_PANEL_SCRIPT = pathlib.Path(__file__).resolve().parent / "ui" / "control_panel.js"
DEFAULT_VIEWPORT = {"width": 1440, "height": 960}


class PlaywrightWrapper:
    def __init__(self, process_manager: ProcessManager):
        self._pm = process_manager
        self._session: aiohttp.ClientSession | None = None
        self._playwright: Playwright | None = None
        self._browser: Browser | None = None
        self._context: BrowserContext | None = None
        self._page: Page | None = None
        self._injected_css: list[str] = []
        self._injected_js: list[str] = []
        self._ui_config = {
            "viewport": dict(DEFAULT_VIEWPORT),
            "zoom": 1.0,
            "host_url": f"{EXTENSION_BASE}/ui/host",
            "console_injected": False,
        }
        self._pm.on_restart(self._on_tui_restart)

    async def start(self):
        headers = {}
        if RUNTIME_TOKEN:
            headers["Authorization"] = f"Bearer {RUNTIME_TOKEN}"
        self._session = aiohttp.ClientSession(headers=headers)
        await self._wait_for_runtime(timeout=600.0)
        await self._launch_browser()
        logger.info("Extension layer connected to TUI.")

    async def stop(self):
        if self._session:
            await self._session.close()
        if self._browser:
            await self._browser.close()
        if self._playwright:
            await self._playwright.stop()

    async def _wait_for_runtime(self, timeout: float = 600.0):
        logger.info(f"Waiting for runtime at {RUNTIME_BASE}  (first run compiles Rust, may take minutes...)")
        deadline = asyncio.get_event_loop().time() + timeout
        attempt = 0
        while asyncio.get_event_loop().time() < deadline:
            try:
                async with self._session.get(
                    f"{RUNTIME_BASE}/health",
                    timeout=aiohttp.ClientTimeout(total=2),
                ) as resp:
                    if resp.status == 200:
                        logger.info(f"TUI alive! (attempt #{attempt})")
                        return
            except Exception:
                pass
            attempt += 1
            if attempt % 30 == 0:
                elapsed = int(timeout - (deadline - asyncio.get_event_loop().time()))
                logger.info(f"Still compiling... ({elapsed}s elapsed)")
            await asyncio.sleep(1.0)
        raise TimeoutError("TUI did not start within 600s")

    async def _on_tui_restart(self):
        await self._wait_for_runtime(timeout=120.0)
        await self._ensure_control_console()

    async def _launch_browser(self):
        self._playwright = await async_playwright().start()
        self._browser = await self._playwright.chromium.launch(headless=False)
        self._context = await self._browser.new_context(viewport=dict(DEFAULT_VIEWPORT))
        self._page = await self._context.new_page()
        await self._page.goto(self._ui_config["host_url"], wait_until="domcontentloaded")
        await self._ensure_control_console()

    async def _ensure_control_console(self):
        if not self._page:
            return
        if self._page.url != self._ui_config["host_url"]:
            await self._page.goto(self._ui_config["host_url"], wait_until="domcontentloaded")

        script = CONTROL_PANEL_SCRIPT.read_text(encoding="utf-8")
        bootstrap = (
            "window.__DEEPSEEK_CONTROL_CONFIG = {"
            f"runtimeBase: '{RUNTIME_BASE}',"
            f"extensionBase: '{EXTENSION_BASE}',"
            f"runtimeToken: '{RUNTIME_TOKEN}'"
            "};"
        )
        await self._page.add_script_tag(content=bootstrap + script)
        self._ui_config["console_injected"] = True
        await self._apply_ui_state()

    async def _require_page(self) -> Page:
        if not self._page:
            raise RuntimeError("Playwright page is not initialized")
        return self._page

    async def _apply_ui_state(self):
        page = await self._require_page()
        await page.set_viewport_size(self._ui_config["viewport"])
        zoom = float(self._ui_config.get("zoom", 1.0))
        await page.evaluate(
            """
            (zoom) => {
                document.documentElement.style.zoom = String(zoom);
                document.body.dataset.deepseekZoom = String(zoom);
            }
            """,
            zoom,
        )
        for css in self._injected_css:
            await page.add_style_tag(content=css)
        for js in self._injected_js:
            await page.add_script_tag(content=js)

    async def get_capabilities(self) -> dict[str, Any]:
        return {
            "process_management": {
                "status": True,
                "start": True,
                "stop": True,
                "restart": True,
                "auto_restart": True,
            },
            "ui_injection": {
                "viewport": True,
                "zoom": True,
                "css": True,
                "javascript": True,
                "reload_console": True,
            },
            "extension_api": {
                "status": "/v1/status",
                "capabilities": "/v1/capabilities",
                "browser_state": "/v1/browser/state",
                "browser_summary": "/v1/browser/summary",
                "browser_dom": "/v1/browser/dom",
                "browser_element": "/v1/browser/element",
                "browser_navigate": "/v1/browser/navigate",
                "browser_screenshot": "/v1/browser/screenshot",
                "browser_evaluate": "/v1/browser/evaluate",
                "browser_ui": "/v1/browser/ui",
                "browser_mcp_task": "/v1/browser/mcp-task",
            },
            "browser_ai_native": {
                "navigate": True,
                "evaluate": True,
                "screenshot": True,
                "dom_injection": True,
                "page_summary": True,
                "dom_inspect": True,
                "element_actions": ["click", "fill", "focus", "highlight"],
                "mcp_browser_task": True,
            },
        }

    async def get_browser_state(self) -> dict[str, Any]:
        page = await self._require_page()
        title = await page.title()
        return {
            "url": page.url,
            "title": title,
            "viewport": dict(self._ui_config["viewport"]),
            "zoom": self._ui_config["zoom"],
            "console_injected": self._ui_config["console_injected"],
            "injected_css_count": len(self._injected_css),
            "injected_js_count": len(self._injected_js),
        }

    async def navigate(self, url: str) -> dict[str, Any]:
        page = await self._require_page()
        await page.goto(url, wait_until="domcontentloaded")
        self._ui_config["host_url"] = url
        await self._apply_ui_state()
        return await self.get_browser_state()

    async def reload_console(self) -> dict[str, Any]:
        self._ui_config["host_url"] = f"{EXTENSION_BASE}/ui/host"
        await self._ensure_control_console()
        return await self.get_browser_state()

    async def update_ui(
        self,
        viewport_width: int | None = None,
        viewport_height: int | None = None,
        zoom: float | None = None,
        css: str | None = None,
        js: str | None = None,
        reset_injections: bool = False,
    ) -> dict[str, Any]:
        if viewport_width is not None or viewport_height is not None:
            width = int(viewport_width or self._ui_config["viewport"]["width"])
            height = int(viewport_height or self._ui_config["viewport"]["height"])
            self._ui_config["viewport"] = {"width": max(width, 320), "height": max(height, 240)}

        if zoom is not None:
            self._ui_config["zoom"] = max(float(zoom), 0.25)

        if reset_injections:
            self._injected_css.clear()
            self._injected_js.clear()

        if css:
            self._injected_css.append(css)
        if js:
            self._injected_js.append(js)

        await self._apply_ui_state()
        return await self.get_browser_state()

    async def evaluate(self, expression: str) -> Any:
        page = await self._require_page()
        return await page.evaluate(expression)

    async def screenshot(self, path: str | None = None) -> dict[str, Any]:
        page = await self._require_page()
        output = pathlib.Path(path) if path else (pathlib.Path(__file__).resolve().parent / "logs" / "playwright-console.png")
        output.parent.mkdir(parents=True, exist_ok=True)
        await page.screenshot(path=str(output), full_page=True)
        return {"path": str(output), "url": page.url}

    async def get_page_summary(self) -> dict[str, Any]:
        page = await self._require_page()
        summary = await page.evaluate(
            """
            () => {
                const text = (document.body?.innerText || "").replace(/\s+/g, " ").trim();
                const headings = Array.from(document.querySelectorAll("h1, h2, h3"))
                    .map((node) => node.textContent?.trim())
                    .filter(Boolean)
                    .slice(0, 8);
                const links = Array.from(document.querySelectorAll("a[href]"))
                    .map((node) => ({
                        text: (node.textContent || "").replace(/\s+/g, " ").trim().slice(0, 80),
                        href: node.getAttribute("href") || ""
                    }))
                    .filter((entry) => entry.text || entry.href)
                    .slice(0, 8);
                return {
                    title: document.title,
                    text_excerpt: text.slice(0, 1200),
                    text_length: text.length,
                    headings,
                    links,
                    forms: document.forms.length,
                    buttons: document.querySelectorAll("button,[role='button']").length,
                    inputs: document.querySelectorAll("input, textarea, select").length,
                };
            }
            """
        )
        return {"url": page.url, **summary}

    async def inspect_dom(self, selector: str) -> dict[str, Any]:
        page = await self._require_page()
        selector = selector.strip()
        if not selector:
            raise RuntimeError("selector is required")
        return await page.evaluate(
            """
            (selector) => {
                const allNodes = Array.from(document.querySelectorAll(selector));
                const nodes = allNodes.slice(0, 12);
                return {
                    selector,
                    count: allNodes.length,
                    matches: nodes.map((node, index) => ({
                        index,
                        tag: node.tagName.toLowerCase(),
                        id: node.id || null,
                        classes: Array.from(node.classList).slice(0, 6),
                        text: (node.textContent || "").replace(/\s+/g, " ").trim().slice(0, 160),
                        value: "value" in node ? String(node.value).slice(0, 160) : null,
                        visible: !!(node.offsetWidth || node.offsetHeight || node.getClientRects().length),
                    })),
                };
            }
            """,
            selector,
        )

    async def operate_element(self, selector: str, action: str, value: str | None = None) -> dict[str, Any]:
        page = await self._require_page()
        selector = selector.strip()
        action = action.strip().lower()
        if not selector:
            raise RuntimeError("selector is required")

        locator = page.locator(selector).first
        await locator.wait_for(timeout=5000)

        if action == "click":
            await locator.click()
        elif action == "fill":
            await locator.fill(value or "")
        elif action == "focus":
            await locator.focus()
        elif action == "highlight":
            await page.evaluate(
                """
                (selector) => {
                    const node = document.querySelector(selector);
                    if (!node) {
                        throw new Error("selector not found");
                    }
                    node.scrollIntoView({ behavior: "smooth", block: "center" });
                    node.dataset.deepseekPreviousOutline = node.style.outline || "";
                    node.style.outline = "3px solid #67e8f9";
                    setTimeout(() => {
                        node.style.outline = node.dataset.deepseekPreviousOutline || "";
                    }, 1800);
                }
                """,
                selector,
            )
        else:
            raise RuntimeError(f"unsupported action: {action}")

        return {
            "ok": True,
            "selector": selector,
            "action": action,
            "value": value,
            "url": page.url,
        }

    async def run_mcp_browser_task(self, prompt: str) -> dict[str, Any]:
        task_prompt = prompt.strip()
        if not task_prompt:
            raise RuntimeError("prompt is required")
        content = await self.send_message(
            "请优先使用当前可用的浏览器 / Playwright / MCP 能力完成以下浏览器任务，必要时先检查页面状态，再执行操作，并返回简洁结果：\n\n"
            f"{task_prompt}"
        )
        return {"ok": True, "prompt": task_prompt, "content": content}

    async def send_message(self, content: str) -> str:
        async with self._session.post(
            f"{RUNTIME_BASE}/v1/stream",
            json={"prompt": content},
            timeout=aiohttp.ClientTimeout(total=300),
        ) as resp:
            resp.raise_for_status()
            event_name = "message"
            chunks: list[str] = []
            async for raw_line in resp.content:
                line = raw_line.decode(errors="replace").strip()
                if not line:
                    continue
                if line.startswith("event:"):
                    event_name = line[6:].strip()
                    continue
                if not line.startswith("data:"):
                    continue

                data = line[5:].strip()
                if event_name == "message.delta":
                    try:
                        parsed = json.loads(data)
                    except Exception:
                        continue
                    delta = parsed.get("content", "")
                    if delta:
                        chunks.append(delta)
                elif event_name == "error":
                    try:
                        parsed = json.loads(data)
                    except Exception:
                        parsed = {"message": data}
                    raise RuntimeError(parsed.get("message", "runtime stream failed"))
            return "".join(chunks).strip()

    async def get_status(self) -> dict:
        async with self._session.get(
            f"{RUNTIME_BASE}/v1/runtime/info",
            timeout=aiohttp.ClientTimeout(total=5),
        ) as runtime_resp:
            runtime = await runtime_resp.json()

        async with self._session.get(
            f"{RUNTIME_BASE}/v1/workspace/status",
            timeout=aiohttp.ClientTimeout(total=5),
        ) as workspace_resp:
            workspace = await workspace_resp.json()

        return {
            "runtime": runtime,
            "workspace": workspace,
            "process": {
                "app_server_running": self._pm.is_alive,
                "app_server_base_url": self._pm.base_url,
                "pid": self._pm.pid,
                "desired_running": self._pm.desired_running,
                "supervising": self._pm.supervising,
                "retry_count": self._pm.retry_count,
            },
            "console": {
                **(await self.get_browser_state() if self._page else {"url": None}),
            },
        }