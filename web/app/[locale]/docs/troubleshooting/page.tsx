import { buildPageMetadata } from "@/lib/page-meta";

export async function generateMetadata({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  const isZh = locale === "zh";
  return buildPageMetadata({
    path: "/docs/troubleshooting",
    locale,
    title: isZh ? "排障 · Codewhale 文档" : "Troubleshooting · Codewhale Docs",
    description: isZh
      ? "常见问题的快速分诊：挂起的回合、离线队列、崩溃恢复、schema 错误、MCP 故障与 Docker 说明。"
      : "Quick triage for common issues: hung turns, the offline queue, crash recovery, schema errors, MCP failures, and Docker notes.",
  });
}

export default async function TroubleshootingPage({
  params,
}: {
  params: Promise<{ locale: string }>;
}) {
  const { locale } = await params;
  const isZh = locale === "zh";
  const bodyClass = isZh
    ? "text-ink-soft leading-[1.9] tracking-wide"
    : "text-ink-soft leading-relaxed";
  const incidents = isZh
    ? [
        {
          name: "回合挂起或流停止",
          detail:
            "前台 shell 命令还在跑时按 Ctrl+B 把它移到后台（回合继续，命令变成 /jobs 下的后台任务）；想取消回合本身用 Esc 或 Ctrl+C。检查 deepseek_cli::client 的重试日志和端点连通性，重启后确认此前在途的回合被标记为中断，而不是停在运行态。",
        },
        {
          name: "网络中断 / 离线行为",
          detail:
            "离线时新提示词会排队，队列持久化在 ~/.codewhale/sessions/checkpoints/offline_queue.json。用 /queue list 查看，恢复连接后重新发送（/queue edit <n> 加回车，或走正常输入流程），队列清空后文件随之清除。",
        },
        {
          name: "崩溃恢复",
          detail:
            "检查点保存在 ~/.codewhale/sessions/checkpoints/latest.json；除非传入 --resume/--continue，启动会开新会话。用 codewhale --resume <id> 或 TUI 里的 Ctrl+R 显式恢复；若检查点 schema 比二进制新，升级二进制或移除过期检查点。",
        },
        {
          name: "持久状态 schema 错误",
          detail:
            "形如 schema vX is newer than supported vY 的错误涉及 sessions、运行时 thread/turn/item 记录和 tasks。先确认二进制版本，编辑前备份状态目录，然后用更新的兼容二进制运行，或归档不兼容记录并重建状态。",
        },
        {
          name: "MCP / 工具执行失败",
          detail:
            "校验 ~/.codewhale/mcp.json 的 schema 和服务器命令路径，手动确认服务器进程能启动，并在 TUI 历史/日志中检查沙箱拒绝。用 /mcp validate 诊断，可暂时禁用出问题的服务器隔离原因，验证后再启用。",
        },
      ]
    : [
        {
          name: "Turn hangs or the stream stops",
          detail:
            "If a foreground shell command is still running, press Ctrl+B to move it to the background (the turn keeps running and the command becomes a background job under /jobs); use Esc or Ctrl+C to cancel the turn itself. Inspect deepseek_cli::client retry logs and endpoint connectivity, and after a restart confirm the previously in-flight turn shows as interrupted rather than running.",
        },
        {
          name: "Network outage / offline behavior",
          detail:
            "New prompts queue while offline, persisted to ~/.codewhale/sessions/checkpoints/offline_queue.json. Inspect with /queue list, restore connectivity, then re-send queued entries (/queue edit <n> plus Enter, or the normal input flow); the queue file clears when the queue empties.",
        },
        {
          name: "Crash recovery",
          detail:
            "The checkpoint lives at ~/.codewhale/sessions/checkpoints/latest.json; startup begins a fresh session unless --resume/--continue is supplied. Resume explicitly with codewhale --resume <id> or Ctrl+R in the TUI; if the checkpoint schema is newer than the binary supports, upgrade the binary or remove the stale checkpoint.",
        },
        {
          name: "Persistent state schema errors",
          detail:
            "Errors like schema vX is newer than supported vY affect sessions, runtime thread/turn/item records, and tasks. Confirm the binary version, back up the state directory before editing, then either run a newer compatible binary or archive the incompatible records and regenerate state.",
        },
        {
          name: "MCP / tool execution failures",
          detail:
            "Validate the ~/.codewhale/mcp.json schema and server command paths, confirm the server process starts manually, and check sandbox denials in TUI history/logs. Use /mcp validate for diagnostics, temporarily disable a failing server to isolate the issue, and re-enable after verification.",
        },
      ];

  return (
    <section className="space-y-10">
      <section id="overview" className="scroll-mt-32">
        <h2 className="font-display text-3xl mb-1">{isZh ? "排障" : "Troubleshooting"}</h2>
        <p className={`${bodyClass} mt-3`}>
          {isZh
            ? "先快速分诊：确认二进制与配置（codewhale --version、~/.codewhale/config.toml），需要更详细日志时用 RUST_LOG=deepseek_cli=debug 启动（HTTP 重试/重连用 RUST_LOG=deepseek_cli::client=debug），并看一眼 ~/.codewhale/sessions 与 ~/.codewhale/tasks 的当前状态。"
            : "Start with quick triage: confirm the binary and config (codewhale --version, ~/.codewhale/config.toml), enable verbose logs with RUST_LOG=deepseek_cli=debug when needed (RUST_LOG=deepseek_cli::client=debug for HTTP retries/reconnects), and capture the current state of ~/.codewhale/sessions and ~/.codewhale/tasks."}
        </p>
        <div className="hairline-t mt-6">
          {incidents.map((row) => (
            <section key={row.name} className="py-4 hairline-b">
              <h3 className="font-display text-lg">{row.name}</h3>
              <p className={`${bodyClass} mt-1 text-sm`}>{row.detail}</p>
            </section>
          ))}
        </div>
      </section>

      <section id="docker" className="scroll-mt-32">
        <h2 className="font-display text-2xl mb-1">{isZh ? "Docker 说明" : "Docker notes"}</h2>
        <p className={`${bodyClass} mt-3`}>
          {isZh
            ? "每个发布都会向 GitHub Container Registry 推送多架构 Linux 镜像。默认镜像是保守的运行时镜像：以非 root 的 codewhale 用户（UID/GID 1000:1000）运行，不授予免密 sudo，用户状态放在挂载到 /home/codewhale/.codewhale 的卷里。可复现的安装请固定发布标签而不是 latest。"
            : "Each release publishes a multi-arch Linux image to GitHub Container Registry. The default image is a conservative runtime image: it runs as the non-root codewhale user (UID/GID 1000:1000), grants no passwordless sudo, and keeps user state in a volume mounted at /home/codewhale/.codewhale. Pin a release tag instead of latest for reproducible installs."}
        </p>
        <pre className="code-block mt-4">{`docker volume create codewhale-home

docker run --rm -it \\
  -e DEEPSEEK_API_KEY="your-api-key-here" \\
  -v codewhale-home:/home/codewhale/.codewhale \\
  -v "$PWD:/workspace" \\
  -w /workspace \\
  ghcr.io/hmbown/codewhale:latest`}</pre>
        <p className={`${bodyClass} mt-3`}>
          {isZh
            ? "需要在容器内使用 apt-get、编译工具链或包管理器时，不要改默认镜像约定——基于 docs/examples/Dockerfile.toolbox 构建显式的 toolbox 镜像，并为每个项目使用独立的命名状态卷，避免会话、配置和离线队列跨工作区串扰。不要把 API 密钥或 SSH 私钥烘进自定义镜像。"
            : "When a project needs apt-get, compiler toolchains, or package managers inside the container, do not change the default image contract — build an explicit toolbox image from docs/examples/Dockerfile.toolbox, and use one named state volume per project so sessions, config, and the offline queue do not bleed across workspaces. Never bake API keys or SSH private keys into custom images."}
        </p>
      </section>

      <section id="source" className="hairline-t pt-8">
        <p className="text-sm text-ink-mute">
          {isZh
            ? "来源文档：docs/OPERATIONS_RUNBOOK.md, docs/DOCKER.md · 更新时请同步修改 docs-map.ts。"
            : "Source documents: docs/OPERATIONS_RUNBOOK.md, docs/DOCKER.md · Update docs-map.ts when changing."}
        </p>
      </section>
    </section>
  );
}
