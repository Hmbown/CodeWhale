//! GUI 集成终端：portable-pty .spawn + Tauri 事件推送 stdout 到前端 xterm

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::Mutex;
use std::thread;

use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use tauri::{AppHandle, Emitter, State};

/// PTY 输出事件（前端 listen `pty-output`）
#[derive(Clone, serde::Serialize)]
struct PtyOutputEvent {
    id: String,
    data: String,
}

/// 单个活跃 PTY 会话
struct ActivePty {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    reader_handle: Option<thread::JoinHandle<()>>,
}

/// 全局 PTY 会话表（Tauri managed state）
pub struct PtyBridge {
    sessions: Mutex<HashMap<String, ActivePty>>,
}

impl Default for PtyBridge {
    fn default() -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
        }
    }
}

impl PtyBridge {
    /// 退出应用时关闭全部 PTY
    pub fn shutdown_all(&self) {
        if let Ok(mut map) = self.sessions.lock() {
            for (_, mut session) in map.drain() {
                let _ = session.child.kill();
                if let Some(h) = session.reader_handle.take() {
                    let _ = h.join();
                }
            }
        }
    }
}

/// 默认交互式 shell 命令
fn build_shell_command() -> CommandBuilder {
    let mut cmd = if cfg!(windows) {
        let mut c = CommandBuilder::new("powershell.exe");
        c.arg("-NoLogo");
        c
    } else {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".into());
        CommandBuilder::new(shell)
    };
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");
    cmd
}

/// 生成唯一 PTY 会话 id
fn new_pty_id() -> String {
    format!(
        "pty-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    )
}

/// 启动 PTY 会话并返回 id
#[tauri::command]
pub fn pty_spawn(
    app: AppHandle,
    bridge: State<PtyBridge>,
    cwd: Option<String>,
    cols: Option<u16>,
    rows: Option<u16>,
) -> Result<String, String> {
    let cols = cols.unwrap_or(120).max(20);
    let rows = rows.unwrap_or(30).max(5);
    let id = new_pty_id();

    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| format!("openpty 失败：{e}"))?;

    let mut cmd = build_shell_command();
    if let Some(dir) = cwd.filter(|d| !d.trim().is_empty()) {
        cmd.cwd(PathBuf::from(dir));
    }

    let child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|e| format!("spawn shell 失败：{e}"))?;
    drop(pair.slave);

    let mut reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| format!("clone reader 失败：{e}"))?;
    let writer = pair
        .master
        .take_writer()
        .map_err(|e| format!("take writer 失败：{e}"))?;

    let app_for_reader = app.clone();
    let reader_id = id.clone();
    let reader_handle = thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    let chunk = String::from_utf8_lossy(&buf[..n]).into_owned();
                    let _ = app_for_reader.emit(
                        "pty-output",
                        PtyOutputEvent {
                            id: reader_id.clone(),
                            data: chunk,
                        },
                    );
                }
                Err(_) => break,
            }
        }
    });

    let session = ActivePty {
        master: pair.master,
        writer,
        child,
        reader_handle: Some(reader_handle),
    };

    bridge
        .sessions
        .lock()
        .map_err(|_| "PTY 锁中毒".to_string())?
        .insert(id.clone(), session);

    Ok(id)
}

/// 向 PTY 写入用户输入
#[tauri::command]
pub fn pty_write(bridge: State<PtyBridge>, id: String, data: String) -> Result<(), String> {
    let mut map = bridge.sessions.lock().map_err(|_| "PTY 锁中毒".to_string())?;
    let session = map
        .get_mut(&id)
        .ok_or_else(|| format!("PTY `{id}` 不存在"))?;
    session
        .writer
        .write_all(data.as_bytes())
        .map_err(|e| format!("写入失败：{e}"))?;
    session.writer.flush().map_err(|e| format!("flush 失败：{e}"))?;
    Ok(())
}

/// 调整 PTY 尺寸
#[tauri::command]
pub fn pty_resize(
    bridge: State<PtyBridge>,
    id: String,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    let map = bridge.sessions.lock().map_err(|_| "PTY 锁中毒".to_string())?;
    let session = map
        .get(&id)
        .ok_or_else(|| format!("PTY `{id}` 不存在"))?;
    session
        .master
        .resize(PtySize {
            rows: rows.max(5),
            cols: cols.max(20),
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| format!("resize 失败：{e}"))?;
    Ok(())
}

/// 关闭 PTY 会话
#[tauri::command]
pub fn pty_close(bridge: State<PtyBridge>, id: String) -> Result<(), String> {
    let mut map = bridge.sessions.lock().map_err(|_| "PTY 锁中毒".to_string())?;
    if let Some(mut session) = map.remove(&id) {
        let _ = session.child.kill();
        if let Some(h) = session.reader_handle.take() {
            let _ = h.join();
        }
    }
    Ok(())
}
