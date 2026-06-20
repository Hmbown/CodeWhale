//! LSP stdio 桥：spawn language server，与前端 @codemirror/lsp-client Transport 对接

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Mutex;

use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;

use super::registry::{Language, detect_language, server_for};

/// 前端 listen 的事件名
pub const LSP_INBOUND_EVENT: &str = "lsp-inbound";

/// 启动会话的返回信息
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LspSessionInfo {
    pub session_id: String,
    pub language_id: String,
    pub root_uri: String,
    pub server_command: String,
}

/// 推送给前端的入站消息
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LspInboundPayload {
    pub session_id: String,
    pub message: String,
}

/// 单个 LSP 会话句柄
struct LspSession {
    /// 出站 JSON（无 Content-Length 头）→ 写 stdin
    tx_outbound: mpsc::UnboundedSender<String>,
    /// 子进程句柄，drop 时 kill_on_drop 生效
    _child: Child,
}

/// 全局 LSP 会话池
pub struct LspBridge {
    sessions: Mutex<HashMap<String, LspSession>>,
}

impl Default for LspBridge {
    fn default() -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
        }
    }
}

impl LspBridge {
    /// 确保 workspace+语言 对应的 LSP 会话已启动
    pub async fn ensure_session(
        &self,
        app: &AppHandle,
        workspace: &str,
        file_path: &str,
    ) -> Result<LspSessionInfo, String> {
        let workspace_path = PathBuf::from(workspace);
        if workspace.trim().is_empty() || !workspace_path.is_dir() {
            return Err("请先打开项目文件夹".to_string());
        }
        let file = PathBuf::from(file_path);
        let lang = detect_language(&file);
        let Some((command, args)) = server_for(lang) else {
            return Err(format!(
                "当前文件类型暂无 LSP 支持（{}）",
                lang.language_id()
            ));
        };

        let session_id = session_id_for(workspace, lang);
        {
            let guard = self.sessions.lock().map_err(|e| e.to_string())?;
            if guard.contains_key(&session_id) {
                return Ok(LspSessionInfo {
                    session_id: session_id.clone(),
                    language_id: lang.language_id().to_string(),
                    root_uri: uri_from_path(&workspace_path),
                    server_command: command.to_string(),
                });
            }
        }

        let mut child = spawn_lsp_process(command, args, &workspace_path)?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| format!("LSP `{command}` 无 stdin"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| format!("LSP `{command}` 无 stdout"))?;

        let (tx_outbound, mut rx_outbound) = mpsc::unbounded_channel::<String>();
        let app_reader = app.clone();
        let session_id_reader = session_id.clone();

        // 写 stdin：前端 JSON → Content-Length 帧
        tauri::async_runtime::spawn(async move {
            let mut stdin = stdin;
            while let Some(json) = rx_outbound.recv().await {
                if write_framed(&mut stdin, json.as_bytes()).await.is_err() {
                    break;
                }
            }
        });

        // 读 stdout：Content-Length 帧 → 前端 JSON 事件
        tauri::async_runtime::spawn(async move {
            read_stdout_loop(stdout, app_reader, session_id_reader).await;
        });

        let session = LspSession {
            tx_outbound,
            _child: child,
        };
        self.sessions
            .lock()
            .map_err(|e| e.to_string())?
            .insert(session_id.clone(), session);

        Ok(LspSessionInfo {
            session_id,
            language_id: lang.language_id().to_string(),
            root_uri: uri_from_path(&workspace_path),
            server_command: command.to_string(),
        })
    }

    /// 转发前端 JSON-RPC 到 LSP stdin
    pub fn send(&self, session_id: &str, message: &str) -> Result<(), String> {
        let guard = self.sessions.lock().map_err(|e| e.to_string())?;
        let session = guard
            .get(session_id)
            .ok_or_else(|| format!("LSP 会话不存在：{session_id}"))?;
        session
            .tx_outbound
            .send(message.to_string())
            .map_err(|_| "LSP 出站通道已关闭".to_string())
    }

    /// 关闭并移除会话
    pub fn stop_session(&self, session_id: &str) -> Result<(), String> {
        let mut guard = self.sessions.lock().map_err(|e| e.to_string())?;
        guard.remove(session_id);
        Ok(())
    }

    /// 应用退出时清理全部 LSP 子进程
    pub fn shutdown_all(&self) {
        if let Ok(mut guard) = self.sessions.lock() {
            guard.clear();
        }
    }
}

/// 生成会话 id：workspace 哈希 + 语言键（JS/TS 共用 typescript-language-server）
fn session_id_for(workspace: &str, lang: Language) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    workspace.to_lowercase().hash(&mut hasher);
    let lang_key = match lang {
        Language::JavaScript => "typescript",
        _ => lang.session_key(),
    };
    format!("{}:{:x}", lang_key, hasher.finish())
}

/// 启动 LSP 子进程（工作目录设为 workspace 根）
fn spawn_lsp_process(
    command: &str,
    args: &[&str],
    workspace: &Path,
) -> Result<Child, String> {
    let mut cmd = Command::new(command);
    cmd.args(args)
        .current_dir(workspace)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    cmd.spawn()
        .map_err(|e| format!("无法启动 LSP `{command}`：{e}（请确认已安装并在 PATH 中）"))
}

/// 将 JSON body 按 LSP Content-Length 协议写入 stdin
async fn write_framed(
    stdin: &mut tokio::process::ChildStdin,
    body: &[u8],
) -> Result<(), ()> {
    let header = format!("Content-Length: {}\r\n\r\n", body.len());
    if stdin.write_all(header.as_bytes()).await.is_err() {
        return Err(());
    }
    if stdin.write_all(body).await.is_err() {
        return Err(());
    }
    if stdin.flush().await.is_err() {
        return Err(());
    }
    Ok(())
}

/// 持续读取 stdout 并 emit 到前端
async fn read_stdout_loop(
    mut stdout: tokio::process::ChildStdout,
    app: AppHandle,
    session_id: String,
) {
    let mut buf: Vec<u8> = Vec::with_capacity(16 * 1024);
    let mut tmp = [0u8; 4096];
    loop {
        let n = match stdout.read(&mut tmp).await {
            Ok(0) => break,
            Ok(n) => n,
            Err(_) => break,
        };
        buf.extend_from_slice(&tmp[..n]);
        while let Some((header_end, content_length)) = parse_header(&buf) {
            if buf.len() < header_end + content_length {
                break;
            }
            let body = &buf[header_end..header_end + content_length];
            let text = String::from_utf8_lossy(body).to_string();
            buf.drain(..header_end + content_length);
            let payload = LspInboundPayload {
                session_id: session_id.clone(),
                message: text,
            };
            let _ = app.emit(LSP_INBOUND_EVENT, payload);
        }
    }
}

/// 解析 Content-Length 头
fn parse_header(buf: &[u8]) -> Option<(usize, usize)> {
    let term = b"\r\n\r\n";
    let pos = buf.windows(term.len()).position(|w| w == term)?;
    let header = std::str::from_utf8(&buf[..pos]).ok()?;
    let mut content_length: Option<usize> = None;
    for line in header.split("\r\n") {
        if let Some(rest) = line.strip_prefix("Content-Length:") {
            content_length = rest.trim().parse().ok();
        }
    }
    content_length.map(|cl| (pos + term.len(), cl))
}

/// 本地路径 → file:// URI（兼容 Windows 盘符）
pub fn uri_from_path(path: &Path) -> String {
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let mut s = canonical.to_string_lossy().replace('\\', "/");
    if s.starts_with("//") {
        return format!("file:{s}");
    }
    if s.len() >= 2 && s.as_bytes()[1] == b':' {
        return format!("file:///{}", s);
    }
    if !s.starts_with('/') {
        s = format!("/{s}");
    }
    format!("file://{s}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_content_length_header() {
        let frame = b"Content-Length: 5\r\n\r\nhello";
        let (end, len) = parse_header(frame).unwrap();
        assert_eq!(end, 21);
        assert_eq!(len, 5);
    }

    #[test]
    fn windows_uri_has_drive() {
        let uri = uri_from_path(Path::new("E:/tmp/foo.rs"));
        assert!(uri.starts_with("file:///"));
    }
}
