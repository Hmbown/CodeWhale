(function () {
    if (window.__deepseekControlConsoleMounted) {
        return;
    }
    window.__deepseekControlConsoleMounted = true;

    const extensionBase = (window.__DEEPSEEK_CONTROL_CONFIG || {}).extensionBase || "http://127.0.0.1:3000";
    let currentZoom = 1;

    const style = document.createElement("style");
    style.textContent = `
        :root {
            color-scheme: dark;
            --panel-bg: rgba(8, 12, 18, 0.82);
            --panel-border: rgba(124, 156, 197, 0.18);
            --panel-shadow: 0 18px 50px rgba(0, 0, 0, 0.38);
            --text-main: #eff6ff;
            --text-muted: #91a4bb;
            --accent: #67e8f9;
            --accent-strong: #22c55e;
            --danger: #fb7185;
            --surface: rgba(15, 23, 34, 0.92);
        }

        body {
            font-family: "Segoe UI Variable Text", "Segoe UI", sans-serif;
            color: var(--text-main);
        }

        #deepseek-control-root {
            position: fixed;
            inset: 0;
            padding: 24px;
            display: grid;
            grid-template-columns: minmax(320px, 380px) minmax(420px, 1fr);
            gap: 20px;
        }

        .ds-card {
            background: var(--panel-bg);
            border: 1px solid var(--panel-border);
            box-shadow: var(--panel-shadow);
            backdrop-filter: blur(18px);
            border-radius: 22px;
        }

        .ds-sidebar {
            padding: 22px;
            display: flex;
            flex-direction: column;
            gap: 18px;
            overflow-y: auto;
        }

        .ds-title {
            font-size: 28px;
            font-weight: 700;
            letter-spacing: 0.02em;
        }

        .ds-subtitle {
            color: var(--text-muted);
            font-size: 13px;
            line-height: 1.6;
        }

        .ds-badge {
            display: inline-flex;
            align-items: center;
            gap: 8px;
            padding: 8px 12px;
            border-radius: 999px;
            background: rgba(103, 232, 249, 0.1);
            color: var(--accent);
            font-size: 12px;
            font-weight: 600;
        }

        .ds-meta {
            display: grid;
            grid-template-columns: repeat(2, minmax(0, 1fr));
            gap: 12px;
        }

        .ds-meta-item {
            padding: 12px 14px;
            border-radius: 16px;
            background: var(--surface);
            min-height: 78px;
        }

        .ds-meta-label {
            color: var(--text-muted);
            font-size: 12px;
            margin-bottom: 6px;
        }

        .ds-meta-value {
            font-size: 15px;
            line-height: 1.5;
            word-break: break-word;
        }

        .ds-meta-value.code {
            font-family: "Cascadia Code", "Consolas", monospace;
            font-size: 13px;
        }

        .ds-actions {
            display: flex;
            flex-wrap: wrap;
            gap: 10px;
        }

        .ds-section {
            display: flex;
            flex-direction: column;
            gap: 12px;
        }

        .ds-section-title {
            font-size: 12px;
            letter-spacing: 0.08em;
            text-transform: uppercase;
            color: var(--text-muted);
        }

        .ds-inline-form {
            display: grid;
            grid-template-columns: 1fr auto;
            gap: 10px;
        }

        .ds-inline-input {
            width: 100%;
            border-radius: 14px;
            border: 1px solid rgba(124, 156, 197, 0.18);
            background: rgba(6, 9, 14, 0.86);
            color: var(--text-main);
            padding: 11px 12px;
            font: inherit;
        }

        .ds-inline-input,
        .ds-input,
        .ds-select {
            box-sizing: border-box;
        }

        .ds-select {
            width: 100%;
            border-radius: 14px;
            border: 1px solid rgba(124, 156, 197, 0.18);
            background: rgba(6, 9, 14, 0.86);
            color: var(--text-main);
            padding: 11px 12px;
            font: inherit;
        }

        .ds-mini-textarea {
            min-height: 88px;
            resize: vertical;
        }

        .ds-pre {
            margin: 0;
            padding: 12px;
            border-radius: 14px;
            background: rgba(6, 9, 14, 0.72);
            border: 1px solid rgba(124, 156, 197, 0.14);
            color: #cfe8ff;
            font-family: "Cascadia Code", "Consolas", monospace;
            font-size: 12px;
            line-height: 1.6;
            overflow: auto;
            white-space: pre-wrap;
            max-height: 220px;
        }

        .ds-tiny-grid {
            display: grid;
            grid-template-columns: repeat(2, minmax(0, 1fr));
            gap: 10px;
        }

        .ds-button {
            border: 0;
            border-radius: 14px;
            padding: 11px 14px;
            font-size: 13px;
            font-weight: 700;
            cursor: pointer;
            color: #041019;
            background: linear-gradient(135deg, #67e8f9, #a7f3d0);
        }

        .ds-button.danger {
            background: linear-gradient(135deg, #fb7185, #fda4af);
            color: #22070b;
        }

        .ds-chat {
            padding: 18px;
            display: grid;
            grid-template-rows: 1fr auto;
            min-height: 0;
        }

        .ds-log {
            overflow: auto;
            padding-right: 6px;
            display: flex;
            flex-direction: column;
            gap: 12px;
        }

        .ds-message {
            max-width: 86%;
            padding: 14px 16px;
            border-radius: 18px;
            line-height: 1.65;
            white-space: pre-wrap;
            background: rgba(15, 23, 34, 0.92);
            border: 1px solid rgba(124, 156, 197, 0.14);
        }

        .ds-message.user {
            align-self: flex-end;
            background: linear-gradient(135deg, rgba(34, 197, 94, 0.22), rgba(103, 232, 249, 0.18));
        }

        .ds-message.assistant {
            align-self: flex-start;
        }

        .ds-composer {
            display: grid;
            grid-template-columns: 1fr auto;
            gap: 12px;
            margin-top: 18px;
        }

        .ds-input {
            min-height: 120px;
            resize: vertical;
            border-radius: 18px;
            border: 1px solid rgba(124, 156, 197, 0.18);
            background: rgba(6, 9, 14, 0.86);
            color: var(--text-main);
            padding: 16px;
            font: inherit;
            line-height: 1.6;
        }

        .ds-input:focus {
            outline: 1px solid rgba(103, 232, 249, 0.5);
        }

        @media (max-width: 980px) {
            #deepseek-control-root {
                grid-template-columns: 1fr;
                grid-template-rows: auto 1fr;
                padding: 14px;
            }
        }
    `;
    document.head.appendChild(style);

    const root = document.createElement("div");
    root.id = "deepseek-control-root";
    root.innerHTML = `
        <aside class="ds-card ds-sidebar">
            <div>
                <div class="ds-badge" id="ds-status-chip">Connecting</div>
            </div>
            <div>
                <div class="ds-title">AI Native Console</div>
                <div class="ds-subtitle">Playwright 注入控制台。统一承接 runtime 状态、扩展层 API 和后续浏览器能力。</div>
            </div>
            <div class="ds-meta">
                <div class="ds-meta-item">
                    <div class="ds-meta-label">Runtime</div>
                    <div class="ds-meta-value" id="ds-runtime">Loading...</div>
                </div>
                <div class="ds-meta-item">
                    <div class="ds-meta-label">Workspace</div>
                    <div class="ds-meta-value" id="ds-workspace">Loading...</div>
                </div>
                <div class="ds-meta-item">
                    <div class="ds-meta-label">Git</div>
                    <div class="ds-meta-value" id="ds-git">Loading...</div>
                </div>
                <div class="ds-meta-item">
                    <div class="ds-meta-label">Managed Process</div>
                    <div class="ds-meta-value" id="ds-app-server">Loading...</div>
                </div>
                <div class="ds-meta-item">
                    <div class="ds-meta-label">Browser</div>
                    <div class="ds-meta-value" id="ds-browser">Loading...</div>
                </div>
                <div class="ds-meta-item">
                    <div class="ds-meta-label">Viewport / Zoom</div>
                    <div class="ds-meta-value" id="ds-viewport">Loading...</div>
                </div>
            </div>
            <div class="ds-section">
                <div class="ds-section-title">Managed App-Server Process</div>
                <div class="ds-subtitle">这组按钮控制的是 Python 扩展层托管的 DeepSeek-TUI app-server，地址是 8787；不是上面单独启动的 7878 runtime API。</div>
                <div class="ds-actions">
                    <button class="ds-button" id="ds-refresh">Refresh</button>
                    <button class="ds-button" id="ds-start">Start</button>
                    <button class="ds-button" id="ds-restart">Restart</button>
                    <button class="ds-button danger" id="ds-stop">Stop</button>
                </div>
            </div>
            <div class="ds-section">
                <div class="ds-section-title">Browser Controls</div>
                <div class="ds-inline-form">
                    <input class="ds-inline-input" id="ds-url" placeholder="https://example.com 或 http://127.0.0.1:3000/ui/host" />
                    <button class="ds-button" id="ds-go">Go</button>
                </div>
                <div class="ds-actions">
                    <button class="ds-button" id="ds-console">Console</button>
                    <button class="ds-button" id="ds-shot">Screenshot</button>
                    <button class="ds-button" id="ds-zoom-in">Zoom +</button>
                    <button class="ds-button" id="ds-zoom-out">Zoom -</button>
                </div>
                <div class="ds-tiny-grid">
                    <input class="ds-inline-input" id="ds-width" placeholder="Width" value="1440" />
                    <input class="ds-inline-input" id="ds-height" placeholder="Height" value="960" />
                </div>
                <div class="ds-actions">
                    <button class="ds-button" id="ds-viewport-apply">Apply Viewport</button>
                </div>
            </div>
            <div class="ds-section">
                <div class="ds-section-title">Browser AI Native</div>
                <div class="ds-actions">
                    <button class="ds-button" id="ds-summary">Page Summary</button>
                </div>
                <pre class="ds-pre" id="ds-summary-output">Summary output will appear here.</pre>
                <div class="ds-inline-form">
                    <input class="ds-inline-input" id="ds-selector" placeholder="CSS selector，例如 button, input[name='q'], a[href]" />
                    <button class="ds-button" id="ds-inspect">Inspect DOM</button>
                </div>
                <pre class="ds-pre" id="ds-dom-output">DOM inspection output will appear here.</pre>
                <div class="ds-tiny-grid">
                    <select class="ds-select" id="ds-element-action">
                        <option value="highlight">highlight</option>
                        <option value="click">click</option>
                        <option value="focus">focus</option>
                        <option value="fill">fill</option>
                    </select>
                    <input class="ds-inline-input" id="ds-element-value" placeholder="fill 时输入内容，其余可留空" />
                </div>
                <div class="ds-actions">
                    <button class="ds-button" id="ds-element-run">Run Element Action</button>
                </div>
                <div class="ds-section-title">MCP Browser Task</div>
                <textarea class="ds-input ds-mini-textarea" id="ds-mcp-task" placeholder="例如：检查当前页面的主要 CTA，并总结是否可点击"></textarea>
                <div class="ds-actions">
                    <button class="ds-button" id="ds-mcp-run">Run MCP Task</button>
                </div>
            </div>
        </aside>
        <section class="ds-card ds-chat">
            <div class="ds-log" id="ds-log"></div>
            <div class="ds-composer">
                <textarea class="ds-input" id="ds-input" placeholder="输入消息，直接走 Python 扩展层转发到 DeepSeek runtime"></textarea>
                <button class="ds-button" id="ds-send">Send</button>
            </div>
        </section>
    `;
    document.body.appendChild(root);

    const statusChip = document.getElementById("ds-status-chip");
    const runtimeNode = document.getElementById("ds-runtime");
    const workspaceNode = document.getElementById("ds-workspace");
    const gitNode = document.getElementById("ds-git");
    const appServerNode = document.getElementById("ds-app-server");
    const browserNode = document.getElementById("ds-browser");
    const viewportNode = document.getElementById("ds-viewport");
    const logNode = document.getElementById("ds-log");
    const inputNode = document.getElementById("ds-input");
    const sendNode = document.getElementById("ds-send");
    const urlNode = document.getElementById("ds-url");
    const widthNode = document.getElementById("ds-width");
    const heightNode = document.getElementById("ds-height");
    const selectorNode = document.getElementById("ds-selector");
    const summaryOutputNode = document.getElementById("ds-summary-output");
    const domOutputNode = document.getElementById("ds-dom-output");
    const elementActionNode = document.getElementById("ds-element-action");
    const elementValueNode = document.getElementById("ds-element-value");
    const mcpTaskNode = document.getElementById("ds-mcp-task");
    const refreshNode = document.getElementById("ds-refresh");
    const startNode = document.getElementById("ds-start");
    const restartNode = document.getElementById("ds-restart");
    const stopNode = document.getElementById("ds-stop");

    function renderJson(node, value) {
        node.textContent = typeof value === "string" ? value : JSON.stringify(value, null, 2);
    }

    function addMessage(role, text) {
        const node = document.createElement("div");
        node.className = `ds-message ${role}`;
        node.textContent = text;
        logNode.appendChild(node);
        logNode.scrollTop = logNode.scrollHeight;
        return node;
    }

    function setProcessButtonsDisabled(disabled) {
        refreshNode.disabled = disabled;
        startNode.disabled = disabled;
        restartNode.disabled = disabled;
        stopNode.disabled = disabled;
    }

    async function loadStatus() {
        const response = await fetch(`${extensionBase}/v1/status`);
        if (!response.ok) {
            throw new Error(`status ${response.status}`);
        }
        const status = await response.json();
        const runtime = status.runtime || {};
        const workspace = status.workspace || {};
        const process = status.process || {};
        const consoleState = status.console || {};

        statusChip.textContent = runtime.auth_required ? "Runtime Ready / Auth" : "Runtime Ready";
        statusChip.style.color = runtime.auth_required ? "#67e8f9" : "#22c55e";
        runtimeNode.textContent = `${runtime.bind_host || "127.0.0.1"}:${runtime.port || "?"}\nversion ${runtime.version || "unknown"}`;
        workspaceNode.textContent = workspace.workspace || "Unknown workspace";
        gitNode.textContent = workspace.git_repo
            ? `${workspace.branch || "detached"}\nstaged ${workspace.staged} / unstaged ${workspace.unstaged} / untracked ${workspace.untracked}`
            : "No git repository";
        appServerNode.textContent = process.app_server_running
            ? `running\npid ${process.pid || "?"}\n${process.app_server_base_url || "http://localhost:8787"}`
            : `stopped\ndesired ${String(process.desired_running)}\n${process.app_server_base_url || "http://localhost:8787"}`;
        appServerNode.classList.add("code");
        browserNode.textContent = `${consoleState.title || "Untitled"}\n${consoleState.url || "No page"}`;
        viewportNode.textContent = `${(consoleState.viewport || {}).width || "?"} x ${(consoleState.viewport || {}).height || "?"}\nzoom ${consoleState.zoom || 1}`;
        currentZoom = Number(consoleState.zoom || 1);
        widthNode.value = String((consoleState.viewport || {}).width || 1440);
        heightNode.value = String((consoleState.viewport || {}).height || 960);
        urlNode.value = consoleState.url || urlNode.value;
    }

    async function sendMessage() {
        const message = inputNode.value.trim();
        if (!message) {
            return;
        }

        inputNode.value = "";
        sendNode.disabled = true;
        addMessage("user", message);
        const replyNode = addMessage("assistant", "Thinking...");

        try {
            const response = await fetch(`${extensionBase}/v1/chat`, {
                method: "POST",
                headers: { "Content-Type": "application/json" },
                body: JSON.stringify({ message })
            });

            if (!response.ok) {
                throw new Error(`chat ${response.status}`);
            }

            const payload = await response.json();
            replyNode.textContent = payload.content || "";
        } catch (error) {
            replyNode.textContent = `Error: ${error}`;
        } finally {
            sendNode.disabled = false;
            inputNode.focus();
        }
    }

    async function postAction(path) {
        setProcessButtonsDisabled(true);
        const response = await fetch(`${extensionBase}${path}`, { method: "POST" });
        try {
            if (!response.ok) {
                throw new Error(`${path} ${response.status}`);
            }
            await loadStatus();
        } finally {
            setProcessButtonsDisabled(false);
        }
    }

    async function postJson(path, payload) {
        const response = await fetch(`${extensionBase}${path}`, {
            method: "POST",
            headers: { "Content-Type": "application/json" },
            body: JSON.stringify(payload || {})
        });
        if (!response.ok) {
            throw new Error(`${path} ${response.status}`);
        }
        return await response.json();
    }

    document.getElementById("ds-refresh").addEventListener("click", () => {
        setProcessButtonsDisabled(true);
        loadStatus()
            .then(() => addMessage("assistant", "Managed app-server status refreshed."))
            .catch((error) => addMessage("assistant", `Status error: ${error}`))
            .finally(() => setProcessButtonsDisabled(false));
    });
    document.getElementById("ds-start").addEventListener("click", () => {
        postAction("/control/start")
            .then(() => addMessage("assistant", "Managed app-server start requested."))
            .catch((error) => addMessage("assistant", `Start error: ${error}`));
    });
    document.getElementById("ds-restart").addEventListener("click", () => {
        postAction("/control/restart")
            .then(() => addMessage("assistant", "Managed app-server restart requested."))
            .catch((error) => addMessage("assistant", `Restart error: ${error}`));
    });
    document.getElementById("ds-stop").addEventListener("click", () => {
        postAction("/control/stop")
            .then(() => addMessage("assistant", "Managed app-server stop requested."))
            .catch((error) => addMessage("assistant", `Stop error: ${error}`));
    });
    document.getElementById("ds-go").addEventListener("click", async () => {
        try {
            const state = await postJson("/v1/browser/navigate", { url: urlNode.value.trim() });
            addMessage("assistant", `Navigated to ${state.url}`);
            await loadStatus();
        } catch (error) {
            addMessage("assistant", `Navigate error: ${error}`);
        }
    });
    document.getElementById("ds-console").addEventListener("click", async () => {
        try {
            await postJson("/v1/browser/reload-console", {});
            addMessage("assistant", "Control console reloaded.");
            await loadStatus();
        } catch (error) {
            addMessage("assistant", `Reload error: ${error}`);
        }
    });
    document.getElementById("ds-shot").addEventListener("click", async () => {
        try {
            const shot = await postJson("/v1/browser/screenshot", {});
            addMessage("assistant", `Screenshot saved to ${shot.path}`);
        } catch (error) {
            addMessage("assistant", `Screenshot error: ${error}`);
        }
    });
    document.getElementById("ds-zoom-in").addEventListener("click", async () => {
        try {
            currentZoom = Number((currentZoom + 0.1).toFixed(2));
            await postJson("/v1/browser/ui", { zoom: currentZoom });
            await loadStatus();
        } catch (error) {
            addMessage("assistant", `Zoom error: ${error}`);
        }
    });
    document.getElementById("ds-zoom-out").addEventListener("click", async () => {
        try {
            currentZoom = Number(Math.max(0.25, currentZoom - 0.1).toFixed(2));
            await postJson("/v1/browser/ui", { zoom: currentZoom });
            await loadStatus();
        } catch (error) {
            addMessage("assistant", `Zoom error: ${error}`);
        }
    });
    document.getElementById("ds-viewport-apply").addEventListener("click", async () => {
        try {
            await postJson("/v1/browser/ui", {
                viewport_width: Number(widthNode.value || 1440),
                viewport_height: Number(heightNode.value || 960)
            });
            addMessage("assistant", "Viewport updated.");
            await loadStatus();
        } catch (error) {
            addMessage("assistant", `Viewport error: ${error}`);
        }
    });
    document.getElementById("ds-summary").addEventListener("click", async () => {
        try {
            const summary = await fetch(`${extensionBase}/v1/browser/summary`).then((response) => {
                if (!response.ok) {
                    throw new Error(`/v1/browser/summary ${response.status}`);
                }
                return response.json();
            });
            renderJson(summaryOutputNode, summary);
            addMessage("assistant", "Page summary captured.");
        } catch (error) {
            addMessage("assistant", `Summary error: ${error}`);
        }
    });
    document.getElementById("ds-inspect").addEventListener("click", async () => {
        try {
            const result = await postJson("/v1/browser/dom", { selector: selectorNode.value.trim() });
            renderJson(domOutputNode, result);
            addMessage("assistant", `DOM inspected for selector: ${result.selector}`);
        } catch (error) {
            addMessage("assistant", `DOM inspect error: ${error}`);
        }
    });
    document.getElementById("ds-element-run").addEventListener("click", async () => {
        try {
            const result = await postJson("/v1/browser/element", {
                selector: selectorNode.value.trim(),
                action: elementActionNode.value,
                value: elementValueNode.value,
            });
            renderJson(domOutputNode, result);
            addMessage("assistant", `Element action executed: ${result.action}`);
            await loadStatus();
        } catch (error) {
            addMessage("assistant", `Element action error: ${error}`);
        }
    });
    document.getElementById("ds-mcp-run").addEventListener("click", async () => {
        try {
            const result = await postJson("/v1/browser/mcp-task", { prompt: mcpTaskNode.value.trim() });
            addMessage("assistant", result.content || "MCP browser task completed.");
        } catch (error) {
            addMessage("assistant", `MCP task error: ${error}`);
        }
    });
    sendNode.addEventListener("click", sendMessage);
    inputNode.addEventListener("keydown", (event) => {
        if (event.key === "Enter" && !event.shiftKey) {
            event.preventDefault();
            sendMessage();
        }
    });

    addMessage("assistant", "Control console attached through Playwright injection.");
    loadStatus().catch((error) => {
        statusChip.textContent = "Runtime Unavailable";
        statusChip.style.color = "#fb7185";
        addMessage("assistant", `Status error: ${error}`);
    });
})();
