# Weixin Bot Bridge

此 bridge 让微信个人账号通过扫码登录控制本地 `codewhale serve --http` runtime。
使用腾讯 iLink Bot 协议（参考 `@tencent-weixin/openclaw-weixin`），
无需公众号注册即可工作。

与现有的 `integrations/wechat-bridge`（公众号客服消息模式）不同，
此 bridge 直接登录**个人微信账号**，通过长轮询 `getUpdates` 收发消息。

## 安全模型

- `codewhale serve --http` 绑定于 `127.0.0.1`。
- `/v1/*` runtime 调用使用 `CODEWHALE_RUNTIME_TOKEN`。
- 微信用户必须加入白名单，除非首次配对时设置 `WEIXIN_ALLOW_UNLISTED=true`。
- 仅支持私聊；暂不支持群聊。
- 工具审批通过文本命令：`/allow <approval_id>` 或 `/deny <approval_id>`。
- bridge 主动向微信服务器发起长轮询请求，无需公网端口。

## 设置

```bash
cd /opt/codewhale/weixin-bot-bridge
npm install --omit=dev
cp .env.example /etc/codewhale/weixin-bot-bridge.env
sudoedit /etc/codewhale/weixin-bot-bridge.env
node src/index.mjs
```

首次启动时会显示一个二维码，用微信扫描以完成登录授权。
登录凭证会自动保存，后续启动无需重新扫码。

## 命令

- `/status`
- `/threads`
- `/new`
- `/resume <thread_id>`
- `/model <name|default>`
- `/interrupt`
- `/compact`
- `/allow <approval_id> [remember]`
- `/deny <approval_id>`

其他所有内容均作为 CodeWhale 提示发送。

## 首次配对

1. 设置 `WEIXIN_ALLOW_UNLISTED=true` 启动 bridge。
2. 扫码登录后，在微信中发送 `/status`。
3. Bridge 会将你的 `user_id` 返回给你（若白名单为空则显示在拒绝消息中）。
4. 将 `user_id` 加入 `WEIXIN_CHAT_ALLOWLIST`。
5. 将 `WEIXIN_ALLOW_UNLISTED` 改回 `false` 并重启 bridge。

## 环境变量

| 变量 | 必填 | 说明 |
|------|------|------|
| `CODEWHALE_RUNTIME_URL` | 否 | Runtime HTTP 地址（默认 `http://127.0.0.1:7878`） |
| `CODEWHALE_RUNTIME_TOKEN` | **是** | Runtime Bearer 令牌 |
| `CODEWHALE_WORKSPACE` | 否 | 工作区路径（默认 cwd） |
| `CODEWHALE_MODEL` | 否 | 模型名称（默认 `auto`） |
| `CODEWHALE_MODE` | 否 | 运行模式（默认 `agent`） |
| `WEIXIN_CHAT_ALLOWLIST` | 否 | 逗号分隔的允许用户 ID |
| `WEIXIN_ALLOW_UNLISTED` | 否 | 首次配对模式（默认 `false`） |
| `WEIXIN_STATE_DIR` | 否 | 状态持久化目录 |
| `WEIXIN_THREAD_MAP_PATH` | 否 | 线程映射文件路径 |
| `WEIXIN_MAX_REPLY_CHARS` | 否 | 单条回复最大字符数（默认 `3500`） |
| `CODEWHALE_TURN_TIMEOUT_MS` | 否 | Turn 超时（默认 `900000`） |
| `WEIXIN_LONGPOLL_TIMEOUT_MS` | 否 | 长轮询超时（默认 `35000`） |

旧的 `WEXIN_*`（拼写错误）变量名仍作为已弃用别名被识别，启动时会打印一次弃用警告。

## 架构

```
微信客户端 ──getUpdates 长轮询──▶ Weixin Bot Bridge ──HTTP──▶ codewhale serve --http
                  ◀──sendMessage──                                  (127.0.0.1:7878)
```

Bridge 通过扫码获取 `bot_token`，然后长轮询 `POST /ilink/bot/getupdates`
以接收消息，并通过 `POST /ilink/bot/sendmessage` 发送回复。
所有消息均带有 `context_token` 以维持会话上下文。

## 与 wechat-bridge 的区别

| 特性 | wechat-bridge | weixin-bot-bridge |
|------|---------------|-------------------|
| 账号类型 | 微信公众号 | 个人微信 |
| 登录方式 | App ID + Secret 配置 | 扫码登录 |
| 消息协议 | 公众号回调 + 客服消息 | iLink Bot 长轮询 + sendMessage |
| 公网需求 | 需要（回调 URL） | 不需要 |
| 消息类型 | 仅文本 | 文本/图片/语音/视频/文件（MVP仅文本） |
