import { buildPageMetadata } from "@/lib/page-meta";

export async function generateMetadata({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  const isZh = locale === "zh";
  return buildPageMetadata({
    path: "/docs/sandbox",
    locale,
    title: isZh ? "沙箱与审批 · Codewhale 文档" : "Sandbox & Approval · Codewhale Docs",
    description: isZh
      ? "macOS Seatbelt、Linux 可选 bubblewrap、平台缺口和审批策略的真实边界。"
      : "The honest boundary: macOS Seatbelt, opt-in Linux bubblewrap, platform gaps, and approval policy.",
  });
}

export default async function SandboxPage({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  const isZh = locale === "zh";
  const bodyClass = isZh
    ? "text-ink-soft leading-[1.9] tracking-wide"
    : "text-ink-soft leading-relaxed";
  const platforms = isZh
    ? [
        {
          name: "macOS · Seatbelt",
          detail:
            "Codewhale 探测 /usr/bin/sandbox-exec；探测成功且策略要求沙箱时，子命令会被包上运行时生成的 Seatbelt profile：广泛的文件系统读取、按策略限制的写入、仅在策略允许时放行网络。探测失败则如实报告无 OS 沙箱。",
        },
        {
          name: "Linux · 可选 bubblewrap",
          detail:
            "Linux 命令沙箱是显式启用的：设置 prefer_bwrap = true，且 /usr/bin/bwrap 是可执行文件时才选用。子命令得到只读根视图，writable 挂载来自解析后的策略；默认隔离网络命名空间，仅在策略开启 network_access 时加 --share-net。未启用或未安装 bwrap 时报告 none。",
        },
        {
          name: "Windows · 无 OS 沙箱",
          detail:
            "Windows 命令路径目前报告无 OS 沙箱。主机权限和审批策略仍然有效，但它们不是 Codewhale 的 OS 命令沙箱。",
        },
        {
          name: "外部 OpenSandbox 执行",
          detail:
            "配置 sandbox_backend = \"opensandbox\" 后，shell 执行会发往配置的 OpenSandbox 兼容 HTTP 端点，而不是启动本地子进程。隔离保证属于所配置的服务及其运营者。",
        },
      ]
    : [
        {
          name: "macOS · Seatbelt",
          detail:
            "Codewhale probes /usr/bin/sandbox-exec; when the probe succeeds and the policy requests a sandbox, the child command is wrapped in a generated Seatbelt profile: broad filesystem reads, policy-limited writes, and network only when the policy enables it. A failed probe is reported honestly as no OS sandbox.",
        },
        {
          name: "Linux · opt-in bubblewrap",
          detail:
            "Linux command sandboxing is opt-in: set prefer_bwrap = true and keep /usr/bin/bwrap executable. The child gets a read-only root view with writable mounts derived from the resolved policy; the network namespace is isolated by default and --share-net is added only when the policy enables network access. Without the opt-in, Codewhale reports none.",
        },
        {
          name: "Windows · no OS sandbox",
          detail:
            "The Windows command path currently reports no OS sandbox. Host permissions and approval policy still apply, but they are not a Codewhale OS command sandbox.",
        },
        {
          name: "External OpenSandbox execution",
          detail:
            "With sandbox_backend = \"opensandbox\", shell execution is sent to the configured OpenSandbox-compatible HTTP endpoint instead of starting a local child. Isolation guarantees belong to the configured service and its operator.",
        },
      ];

  return (
    <section className="space-y-10">
      <section id="overview" className="scroll-mt-32">
        <h2 className="font-display text-3xl mb-1">{isZh ? "沙箱与审批" : "Sandbox & Approval"}</h2>
        <p className={`${bodyClass} mt-3`}>
          {isZh
            ? "Codewhale 可以启动由模型提出的 shell 命令。审批策略、感知工作区的文件工具和操作系统命令包装器是三个独立的控制：一次审批不是沙箱，选择 workspace-write 也不代表当前平台有可用的 OS 包装器。本页只描述已经接入命令执行路径的行为。"
            : "Codewhale can launch shell commands proposed by a model. Approval policy, workspace-aware tools, and an operating-system command wrapper are separate controls: an approval is not a sandbox, and selecting workspace-write does not prove the current platform has an OS wrapper available. This page describes only behavior wired into the command execution path."}
        </p>
        <div className="hairline-t mt-6">
          {platforms.map((row) => (
            <section key={row.name} className="py-4 hairline-b">
              <h3 className="font-display text-lg">{row.name}</h3>
              <p className={`${bodyClass} mt-1 text-sm`}>{row.detail}</p>
            </section>
          ))}
        </div>
      </section>

      <section id="policies" className="scroll-mt-32">
        <h2 className="font-display text-2xl mb-1">{isZh ? "策略与回退" : "Policies and fallbacks"}</h2>
        <p className={`${bodyClass} mt-3`}>
          {isZh ? (
            <>
              本地 <code className="inline">sandbox_mode</code> 取值为{" "}
              <code className="inline">read-only</code>、<code className="inline">workspace-write</code>、
              <code className="inline">danger-full-access</code> 或{" "}
              <code className="inline">external-sandbox</code>。前两者只在选中且可用的 Seatbelt 或
              bubblewrap 包装器下被强制执行；<code className="inline">danger-full-access</code>{" "}
              有意绕过本地 OS 包装器；<code className="inline">external-sandbox</code>{" "}
              声明执行已被外部隔离。没有选中包装器时，shell 命令在没有 Codewhale OS 隔离的情况下运行——审批规则和感知工作区的原生文件工具仍是独立的控制。
            </>
          ) : (
            <>
              The local <code className="inline">sandbox_mode</code> values are{" "}
              <code className="inline">read-only</code>, <code className="inline">workspace-write</code>,{" "}
              <code className="inline">danger-full-access</code>, and{" "}
              <code className="inline">external-sandbox</code>. The first two are enforced by Seatbelt or
              bubblewrap only when that wrapper is selected and available;{" "}
              <code className="inline">danger-full-access</code> deliberately bypasses the local OS
              wrapper; <code className="inline">external-sandbox</code> declares that execution is already
              externally isolated. When no wrapper is selected, the shell command runs without Codewhale
              OS isolation — approval rules and workspace-aware native file tools remain separate controls.
            </>
          )}
        </p>
        <pre className="code-block mt-4">{`# config.toml
sandbox_mode = "workspace-write"
prefer_bwrap = true            # Linux opt-in

# Canonical environment overrides
CODEWHALE_SANDBOX_MODE
CODEWHALE_SANDBOX_BACKEND
CODEWHALE_SANDBOX_URL
CODEWHALE_SANDBOX_API_KEY`}</pre>
      </section>

      <section id="diagnostics" className="scroll-mt-32">
        <h2 className="font-display text-2xl mb-1">{isZh ? "诊断与限制" : "Diagnostics and limits"}</h2>
        <p className={`${bodyClass} mt-3`}>
          {isZh
            ? "codewhale setup --status、codewhale doctor、codewhale doctor --json 和 diagnostics 工具会报告应用 bubblewrap 偏好后本地可用的包装器。拒绝归因是保守的：子命令的通用 Permission denied 本身并不能证明是 Codewhale 的沙箱拦截了它，未沙箱化的命令失败永远不会被标记为沙箱拒绝。"
            : "codewhale setup --status, codewhale doctor, codewhale doctor --json, and the diagnostics tool report the locally available wrapper after applying the resolved bubblewrap preference. Denial attribution is intentionally conservative: a child command's generic Permission denied is not by itself proof that Codewhale's sandbox blocked it, and unsandboxed command failures are never labeled sandbox denials."}
        </p>
        <p className={`${bodyClass} mt-3`}>
          {isZh
            ? "限制同样如实说明：可用性在启动前检查，选中的包装器仍可能因主机策略、容器限制或竞态而失败；bubblewrap 会忽略缺失或不是目录的可写根；没有任何沙箱能防御内核漏洞或所有资源耗尽与侧信道攻击。"
            : "The limitations are stated just as plainly: availability is checked before launch, yet the selected wrapper can still fail because of host policy, container restrictions, or a race after the probe; bubblewrap ignores a configured writable root that is missing or not a directory; and no sandbox protects against kernel vulnerabilities or all resource-exhaustion and side-channel attacks."}
        </p>
      </section>

      <section id="source" className="hairline-t pt-8">
        <p className="text-sm text-ink-mute">
          {isZh
            ? "来源文档：docs/SANDBOX.md · 更新时请同步修改 docs-map.ts。"
            : "Source document: docs/SANDBOX.md · Update docs-map.ts when changing."}
        </p>
      </section>
    </section>
  );
}
