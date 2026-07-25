import { buildPageMetadata } from "@/lib/page-meta";

export async function generateMetadata({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  const isZh = locale === "zh";
  return buildPageMetadata({
    path: "/docs/web",
    locale,
    title: isZh ? "浏览器客户端 · Codewhale 文档" : "Browser Client · Codewhale Docs",
    description: isZh
      ? "仅回环的内嵌浏览器客户端：一次性引导、会话 Cookie 与本地信任边界。"
      : "The loopback-only embedded browser client: one-time bootstrap, session cookie, and the local trust boundary.",
  });
}

export default async function WebClientPage({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  const isZh = locale === "zh";
  const bodyClass = isZh
    ? "text-ink-soft leading-[1.9] tracking-wide"
    : "text-ink-soft leading-relaxed";

  return (
    <section className="space-y-10">
      <section id="overview" className="scroll-mt-32">
        <h2 className="font-display text-3xl mb-1">{isZh ? "浏览器客户端" : "Browser Client"}</h2>
        <p className={`${bodyClass} mt-3`}>
          {isZh ? (
            <>
              <code className="inline">codewhale web</code> 在 canonical 运行时 API
              之上打开 Codewhale 内嵌的浏览器客户端。它是一个纯本地界面：服务器始终绑定{" "}
              <code className="inline">127.0.0.1</code>，无法改绑到局域网地址，也无法在关闭运行时认证的情况下运行。默认地址是{" "}
              <code className="inline">http://127.0.0.1:7878</code>；端口冲突时用{" "}
              <code className="inline">codewhale web --port 8788</code>{" "}
              换一个回环端口。Ctrl+C 停止进程，浏览器会话随之结束。
            </>
          ) : (
            <>
              <code className="inline">codewhale web</code> opens Codewhale's embedded browser client
              over the canonical Runtime API. It is a local surface: the server always binds to{" "}
              <code className="inline">127.0.0.1</code>, cannot be rebound to a LAN address, and cannot
              run with Runtime authentication disabled. The default address is{" "}
              <code className="inline">http://127.0.0.1:7878</code>; on a port collision, pick another
              loopback port with <code className="inline">codewhale web --port 8788</code>. Stop the
              process with Ctrl+C and the browser session ends with it.
            </>
          )}
        </p>
        <p className={`${bodyClass} mt-3`}>
          {isZh
            ? "当前客户端提供响应式的线程与搜索侧栏、由运行时持有的会话事实、transcript 与工具收据，以及输入区。它可以创建、选择、重命名和归档线程；发起或引导回合；中断工作；处理审批；回答运行时的用户输入请求。浏览器只是同一个本地运行时的另一视图——不会创建第二个云账号，不会把 provider 凭据复制进浏览器存储，也不会削弱已配置的审批与沙箱策略。"
            : "The current client provides a responsive thread and search rail, Runtime-owned session facts, transcript and tool receipts, and a composer. It can create, select, rename, and archive threads; start or steer turns; interrupt work; resolve approvals; and answer Runtime user-input requests. The browser is another view of the same local Runtime — it does not create a second cloud account, copy provider credentials into browser storage, or weaken the configured approval and sandbox policies."}
        </p>
      </section>

      <section id="auth" className="scroll-mt-32">
        <h2 className="font-display text-2xl mb-1">{isZh ? "认证边界" : "Authentication boundary"}</h2>
        <p className={`${bodyClass} mt-3`}>
          {isZh
            ? "启动 URL 携带的是一个随机、短寿命、一次性的引导凭证——绝不是运行时 bearer 令牌。一次回环请求把它换成 HttpOnly、SameSite=Strict、进程本地的会话 Cookie，并立即使该凭证失效。重用、过期、畸形或非回环的引导尝试都会失败关闭。运行时令牌不会出现在渲染的 HTML、浏览器存储、URL 查询或片段、或浏览器启动参数中。携带 Cookie 的状态变更请求还必须出示精确的本地 web 源；跨源浏览器请求会被拒绝。"
            : "The browser-launch URL carries a random, short-lived, one-time bootstrap capability — never the Runtime bearer token. A loopback request exchanges it for an HttpOnly, SameSite=Strict, process-local session cookie and immediately invalidates the capability. Reused, expired, malformed, and non-loopback bootstrap attempts fail closed. The Runtime token is never placed in rendered HTML, browser storage, URL queries or fragments, or browser-launch arguments. Cookie-authenticated state-changing requests must also present the exact local web origin; cross-origin browser requests are rejected."}
        </p>
      </section>

      <section id="local" className="scroll-mt-32">
        <h2 className="font-display text-2xl mb-1">{isZh ? "本地就是本地" : "Local means local"}</h2>
        <p className={`${bodyClass} mt-3`}>
          {isZh ? (
            <>
              <code className="inline">codewhale web</code> 只接受{" "}
              <code className="inline">--port</code>——没有 <code className="inline">--host</code>
              ，也没有关闭认证的选项。不要把它当公开网站，也不要通过路由器转发、公开反向代理或隧道暴露它的端口。单独的{" "}
              <code className="inline">codewhale app-server --mobile</code> 和{" "}
              <code className="inline">--http</code>{" "}
              模式有不同的部署与认证约定，操作它们（尤其是选择非回环绑定）之前请阅读运行时 API 文档。
            </>
          ) : (
            <>
              <code className="inline">codewhale web</code> accepts only{" "}
              <code className="inline">--port</code> — there is no <code className="inline">--host</code>{" "}
              and no insecure-auth option on this command. Do not treat it as a public website or expose
              its port through router forwarding, a public reverse proxy, or a tunnel. The separate{" "}
              <code className="inline">codewhale app-server --mobile</code> and{" "}
              <code className="inline">--http</code> modes carry different deployment and authentication
              contracts; read the Runtime API documentation before operating either one, especially
              before selecting a non-loopback bind.
            </>
          )}
        </p>
      </section>

      <section id="troubleshooting" className="scroll-mt-32">
        <h2 className="font-display text-2xl mb-1">{isZh ? "常见问题" : "Troubleshooting"}</h2>
        <p className={`${bodyClass} mt-3`}>
          {isZh
            ? "端口 7878 被占用时用 --port 换一个。浏览器无法打开时命令会报错退出，而不会留下可重用的引导凭证；检查系统默认浏览器设置后重新启动。页面能打开但 provider 不可用时，查 codewhale doctor 和 /provider——web 命令不配置也不迁移 provider 凭据。会话过期后重启 codewhale web 以签发新的进程本地会话；重用旧的引导 URL 本来就会失败。"
            : "If port 7878 is occupied, pass an unused --port. If the browser cannot be opened, the command exits with an error rather than leaving a reusable bootstrap capability behind; check the OS default-browser setup and start again. If the page loads but a provider is unavailable, inspect codewhale doctor and /provider — the web command does not configure or move provider credentials. If a session expired, restart codewhale web to mint a new process-local session; reusing an old bootstrap URL is expected to fail."}
        </p>
      </section>

      <section id="source" className="hairline-t pt-8">
        <p className="text-sm text-ink-mute">
          {isZh
            ? "来源文档：docs/WEB.md · 更新时请同步修改 docs-map.ts。"
            : "Source document: docs/WEB.md · Update docs-map.ts when changing."}
        </p>
      </section>
    </section>
  );
}
