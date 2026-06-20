// DeepSeek GUI 的 Tauri 壳入口
// 职责：
//   1. 启动时以子进程方式拉起后端 `deepseek-tui serve --http`（sidecar）。
//   2. 退出时清理后端子进程。
//   3. 暴露给前端的命令：读写 API Key（落到 ~/.deepseek/config.toml）、
//      重启后端、查询后端二进制与配置状态。
// 后端二进制解析顺序：环境变量 DEEPSEEK_SERVE_BIN → 与本程序同目录的
// deepseek-tui(.exe) → PATH 上的 deepseek-tui。

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod config_bridge;
mod lsp;
mod pty;

use config_bridge::{
    add_trust, init_mcp_config, read_anchors, read_hooks_config, read_mcp_config, read_memory,
    read_network_config, read_notes, read_subagent_state, read_trust, remove_trust, save_anchors,
    save_hooks_config, save_mcp_config, save_memory, save_network_config, save_notes,
};

use std::path::PathBuf;
use std::process::{Child, Command};
use std::sync::Mutex;

use tauri::{AppHandle, Manager, RunEvent, State};

use lsp::LspBridge;
use pty::{PtyBridge, pty_close, pty_resize, pty_spawn, pty_write};

/// 后端进程管理状态：固定 token + 子进程句柄 + 工作目录（agent 读写文件的根）
struct Backend {
    token: String,
    child: Mutex<Option<Child>>,
    /// 后端工作目录：agent 的文件工具相对此目录读写。None 表示用默认（程序目录）。
    workspace: Mutex<Option<String>>,
}

/// 解析后端可执行文件路径
fn resolve_backend_bin() -> String {
    // 1. 环境变量显式指定
    if let Ok(p) = std::env::var("DEEPSEEK_SERVE_BIN") {
        if !p.trim().is_empty() {
            return p;
        }
    }
    // 2. 与本程序同目录的 deepseek-tui(.exe)
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let name = if cfg!(windows) {
                "deepseek-tui.exe"
            } else {
                "deepseek-tui"
            };
            let sibling = dir.join(name);
            if sibling.exists() {
                return sibling.to_string_lossy().to_string();
            }
        }
    }
    // 3. 退回 PATH 查找
    "deepseek-tui".to_string()
}

/// 启动后端子进程；失败返回 None（前端会显示离线并轮询重试）。
/// workspace 为 Some 时，将其设为后端进程的工作目录，使 agent 的文件读写落在该目录。
fn spawn_backend(token: &str, workspace: Option<&str>) -> Option<Child> {
    let bin = resolve_backend_bin();
    // WebView2 在 Windows 上的页面来源是 http(s)://tauri.localhost，
    // 在 macOS/Linux 上是 tauri://localhost。后端默认 CORS 白名单只含
    // tauri://localhost，这里补充 http(s)://tauri.localhost，避免被 CORS 拦截。
    let cors_origins = "http://tauri.localhost,https://tauri.localhost,tauri://localhost";
    let mut cmd = Command::new(&bin);
    cmd.args(["serve", "--http"])
        .env("DEEPSEEK_RUNTIME_TOKEN", token)
        .env("DEEPSEEK_CORS_ORIGINS", cors_origins);
    // 设置工作目录（agent 文件工具的根目录）
    if let Some(dir) = workspace {
        if !dir.trim().is_empty() {
            cmd.current_dir(dir);
        }
    }
    match cmd.spawn() {
        Ok(child) => {
            eprintln!(
                "[gui] backend started: {bin} serve --http (cwd={})",
                workspace.unwrap_or("default")
            );
            Some(child)
        }
        Err(err) => {
            eprintln!("[gui] 未能启动后端 `{bin} serve --http`：{err}");
            None
        }
    }
}

/// 解析 ~/.deepseek/config.toml 路径
fn config_path() -> Option<PathBuf> {
    let home = std::env::var("USERPROFILE")
        .ok()
        .or_else(|| std::env::var("HOME").ok())?;
    Some(PathBuf::from(home).join(".deepseek").join("config.toml"))
}

/// 命令：返回前端需要的连接信息（后端固定 token、配置文件路径）
#[tauri::command]
fn get_settings() -> serde_json::Value {
    let path = config_path()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();
    serde_json::json!({
        "config_path": path,
        "api_key_present": api_key_present_inner(),
    })
}

/// 命令：返回后端固定 token，供前端 API 客户端使用
#[tauri::command]
fn get_runtime_token(state: State<Backend>) -> String {
    state.token.clone()
}

/// 判断 API Key 是否已配置（环境变量或 config.toml）
fn api_key_present_inner() -> bool {
    if std::env::var("DEEPSEEK_API_KEY")
        .map(|v| !v.trim().is_empty())
        .unwrap_or(false)
    {
        return true;
    }
    if let Some(p) = config_path() {
        if let Ok(s) = std::fs::read_to_string(&p) {
            if let Ok(v) = s.parse::<toml::Value>() {
                return v
                    .get("api_key")
                    .and_then(|x| x.as_str())
                    .map(|s| !s.trim().is_empty())
                    .unwrap_or(false);
            }
        }
    }
    false
}

/// 命令：把 API Key 写入 config.toml（保留其他已有配置）
#[tauri::command]
fn save_api_key(key: String) -> Result<(), String> {
    let key = key.trim().to_string();
    if key.is_empty() {
        return Err("API Key 不能为空".to_string());
    }
    let p = config_path().ok_or_else(|| "无法定位配置目录".to_string())?;
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    // 读取已有配置（不存在则空表），仅更新 api_key 字段
    let mut doc: toml::Value = std::fs::read_to_string(&p)
        .ok()
        .and_then(|s| s.parse::<toml::Value>().ok())
        .unwrap_or_else(|| toml::Value::Table(Default::default()));
    if let toml::Value::Table(ref mut t) = doc {
        t.insert("api_key".to_string(), toml::Value::String(key));
    }
    let out = toml::to_string_pretty(&doc).map_err(|e| e.to_string())?;
    std::fs::write(&p, out).map_err(|e| e.to_string())?;
    Ok(())
}

/// 命令：读取当前配置（api_key 仅返回是否已配置，不回传明文）
#[tauri::command]
fn get_config() -> serde_json::Value {
    let mut out = serde_json::Map::new();
    if let Some(p) = config_path() {
        if let Ok(s) = std::fs::read_to_string(&p) {
            if let Ok(toml::Value::Table(t)) = s.parse::<toml::Value>() {
                for k in [
                    "base_url",
                    "provider",
                    "default_text_model",
                    "reasoning_effort",
                ] {
                    if let Some(v) = t.get(k).and_then(|x| x.as_str()) {
                        out.insert(k.to_string(), serde_json::Value::String(v.to_string()));
                    }
                }
                if let Some(b) = t.get("allow_shell").and_then(|x| x.as_bool()) {
                    out.insert("allow_shell".to_string(), serde_json::Value::Bool(b));
                }
            }
        }
    }
    out.insert(
        "api_key_present".to_string(),
        serde_json::Value::Bool(api_key_present_inner()),
    );
    out.insert(
        "config_path".to_string(),
        serde_json::Value::String(
            config_path()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default(),
        ),
    );
    serde_json::Value::Object(out)
}

/// 命令：把一组配置项合并写入 config.toml（保留其他已有配置）。
/// 字符串为空表示删除该键；布尔直接写入；前端不传的键保持不变。
#[tauri::command]
fn save_config(patch: serde_json::Value) -> Result<(), String> {
    let p = config_path().ok_or_else(|| "无法定位配置目录".to_string())?;
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let mut doc: toml::Value = std::fs::read_to_string(&p)
        .ok()
        .and_then(|s| s.parse::<toml::Value>().ok())
        .unwrap_or_else(|| toml::Value::Table(Default::default()));
    let toml::Value::Table(ref mut t) = doc else {
        return Err("配置文件格式错误".to_string());
    };
    if let serde_json::Value::Object(obj) = patch {
        for (k, v) in obj {
            match v {
                serde_json::Value::String(s) => {
                    let s = s.trim().to_string();
                    if s.is_empty() {
                        t.remove(&k);
                    } else {
                        t.insert(k, toml::Value::String(s));
                    }
                }
                serde_json::Value::Bool(b) => {
                    t.insert(k, toml::Value::Boolean(b));
                }
                serde_json::Value::Null => {
                    t.remove(&k);
                }
                _ => {}
            }
        }
    }
    let out = toml::to_string_pretty(&doc).map_err(|e| e.to_string())?;
    std::fs::write(&p, out).map_err(|e| e.to_string())?;
    Ok(())
}

// ===================== 文件系统命令（供三栏 IDE 布局使用）=====================

/// 目录项：文件树渲染所需的最小信息
#[derive(serde::Serialize)]
struct DirEntryInfo {
    /// 文件/目录名
    name: String,
    /// 绝对路径
    path: String,
    /// 是否为目录
    is_dir: bool,
}

/// 读取文件返回结构
#[derive(serde::Serialize)]
struct FileContent {
    /// 文件文本内容（按 UTF-8 有损解码）
    content: String,
    /// 是否因超过大小上限而被截断
    truncated: bool,
    /// 是否疑似二进制文件
    binary: bool,
}

/// 读取单个文件的大小上限（2 MiB），超出则截断，避免卡死渲染
const MAX_READ_BYTES: usize = 2 * 1024 * 1024;

/// 命令：弹出原生「选择文件夹」对话框，返回所选目录绝对路径
#[tauri::command]
fn pick_folder() -> Option<String> {
    rfd::FileDialog::new()
        .pick_folder()
        .map(|p| p.to_string_lossy().to_string())
}

/// 命令：弹出原生「选择文件」对话框（/attach 等）
#[tauri::command]
fn pick_file() -> Option<String> {
    rfd::FileDialog::new()
        .pick_file()
        .map(|p| p.to_string_lossy().to_string())
}

/// 命令：列出目录下一层条目（目录在前、按名称排序）。
/// 用于文件树的惰性展开，每次只读一层，避免深递归卡顿。
#[tauri::command]
fn list_dir(path: String) -> Result<Vec<DirEntryInfo>, String> {
    let rd = std::fs::read_dir(&path).map_err(|e| format!("读取目录失败：{e}"))?;
    let mut entries: Vec<DirEntryInfo> = Vec::new();
    for item in rd.flatten() {
        let p = item.path();
        let name = item.file_name().to_string_lossy().to_string();
        // 跳过常见的重型/隐藏目录，保持文件树清爽
        if matches!(name.as_str(), ".git" | "node_modules" | "target" | "dist") {
            continue;
        }
        let is_dir = p.is_dir();
        entries.push(DirEntryInfo {
            name,
            path: p.to_string_lossy().to_string(),
            is_dir,
        });
    }
    // 目录优先，其次按名称不区分大小写排序
    entries.sort_by(|a, b| match (a.is_dir, b.is_dir) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
    });
    Ok(entries)
}

/// 命令：读取文件内容（有损 UTF-8 解码 + 大小上限 + 二进制探测）
#[tauri::command]
fn read_file(path: String) -> Result<FileContent, String> {
    let bytes = std::fs::read(&path).map_err(|e| format!("读取文件失败：{e}"))?;
    // 二进制探测：前 8KiB 内出现 NUL 字节则视为二进制
    let probe = &bytes[..bytes.len().min(8192)];
    let binary = probe.contains(&0u8);
    if binary {
        return Ok(FileContent {
            content: String::new(),
            truncated: false,
            binary: true,
        });
    }
    let truncated = bytes.len() > MAX_READ_BYTES;
    let slice = &bytes[..bytes.len().min(MAX_READ_BYTES)];
    Ok(FileContent {
        content: String::from_utf8_lossy(slice).to_string(),
        truncated,
        binary: false,
    })
}

/// 命令：将文本内容写入指定文件（覆盖写入，UTF-8）。
/// 用于 GUI 代码编辑器保存；路径不存在时由调用方保证父目录已存在。
#[tauri::command]
fn write_file(path: String, content: String) -> Result<(), String> {
    let p = std::path::Path::new(&path);
    if !p.exists() {
        return Err("文件不存在".to_string());
    }
    if p.is_dir() {
        return Err("目标是目录，无法写入".to_string());
    }
    std::fs::write(p, content.as_bytes()).map_err(|e| format!("写入文件失败：{e}"))
}

/// 命令：删除指定路径的文件或目录（目录递归删除）。
/// 返回前会做基本安全校验，避免误删盘根等危险路径。
#[tauri::command]
fn delete_path(path: String) -> Result<(), String> {
    let p = std::path::Path::new(&path);
    if !p.exists() {
        return Err("路径不存在".to_string());
    }
    // 安全校验：拒绝删除盘根（如 C:\ 或 / 等组件数过少的路径），避免灾难性误删
    let comp_count = p.components().count();
    if comp_count <= 2 {
        return Err("出于安全考虑，拒绝删除根级路径".to_string());
    }
    // 目录递归删除，文件直接删除
    let result = if p.is_dir() {
        std::fs::remove_dir_all(p)
    } else {
        std::fs::remove_file(p)
    };
    result.map_err(|e| format!("删除失败：{e}"))
}

/// 命令：新建文件（可选初始内容；父目录不存在时自动创建）。
#[tauri::command]
fn create_file(path: String, content: Option<String>) -> Result<(), String> {
    let p = std::path::Path::new(&path);
    if p.exists() {
        return Err("路径已存在".to_string());
    }
    if let Some(parent) = p.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|e| format!("创建父目录失败：{e}"))?;
        }
    }
    let body = content.unwrap_or_default();
    std::fs::write(p, body.as_bytes()).map_err(|e| format!("创建文件失败：{e}"))
}

/// 命令：新建文件夹（含中间层级）。
#[tauri::command]
fn create_dir(path: String) -> Result<(), String> {
    let p = std::path::Path::new(&path);
    if p.exists() {
        return Err("路径已存在".to_string());
    }
    std::fs::create_dir_all(p).map_err(|e| format!("创建文件夹失败：{e}"))
}

/// 命令：重命名/移动文件或文件夹。
#[tauri::command]
fn rename_path(from: String, to: String) -> Result<(), String> {
    let src = std::path::Path::new(&from);
    let dst = std::path::Path::new(&to);
    if !src.exists() {
        return Err("源路径不存在".to_string());
    }
    if dst.exists() {
        return Err("目标路径已存在".to_string());
    }
    if let Some(parent) = dst.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|e| format!("创建目标父目录失败：{e}"))?;
        }
    }
    std::fs::rename(src, dst).map_err(|e| format!("重命名失败：{e}"))
}

/// 命令：获取工作区的 git 变更（未暂存 + 已暂存）合并文本。
/// 用于 GUI 内「查看全部变更」(/diff)。无 git 或无变更时返回友好提示。
#[tauri::command]
fn git_diff(dir: String) -> Result<String, String> {
    let workdir = std::path::Path::new(&dir);
    if dir.trim().is_empty() || !workdir.exists() {
        return Err("请先打开一个项目文件夹".to_string());
    }
    // 分别取未暂存与已暂存的 diff
    let run = |args: &[&str]| -> Result<String, String> {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(workdir)
            .output()
            .map_err(|e| format!("执行 git 失败：{e}（请确认已安装 git）"))?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            return Err(format!("git 命令出错：{err}"));
        }
        Ok(String::from_utf8_lossy(&out.stdout).to_string())
    };
    let unstaged = run(&["diff"])?;
    let staged = run(&["diff", "--staged"])?;
    let mut combined = String::new();
    if !staged.trim().is_empty() {
        combined.push_str("# 已暂存的变更 (staged)\n");
        combined.push_str(&staged);
        combined.push('\n');
    }
    if !unstaged.trim().is_empty() {
        combined.push_str("# 未暂存的变更 (unstaged)\n");
        combined.push_str(&unstaged);
    }
    if combined.trim().is_empty() {
        return Ok("（工作区没有 git 变更）".to_string());
    }
    Ok(combined)
}

/// 命令：保存附件（如粘贴的图片）到工作区 .deepseek/attachments 目录，返回绝对路径。
/// GUI 内粘贴图片后保存为文件，再以 @相对路径 让 agent 通过工具读取/分析。
#[tauri::command]
fn save_attachment(dir: String, name: String, bytes: Vec<u8>) -> Result<String, String> {
    // 基目录：优先用传入工作区，否则退回临时目录
    let base = if dir.trim().is_empty() {
        std::env::temp_dir()
    } else {
        std::path::Path::new(&dir).to_path_buf()
    };
    let att_dir = base.join(".deepseek").join("attachments");
    std::fs::create_dir_all(&att_dir).map_err(|e| format!("创建附件目录失败：{e}"))?;
    // 文件名净化，避免路径穿越
    let safe = name
        .chars()
        .map(|c| if "\\/:*?\"<>|".contains(c) { '_' } else { c })
        .collect::<String>();
    let target = att_dir.join(&safe);
    std::fs::write(&target, &bytes).map_err(|e| format!("写入附件失败：{e}"))?;
    Ok(target.to_string_lossy().to_string())
}

/// 从 config.toml 读取某个字符串配置项
fn config_str(key: &str) -> Option<String> {
    let p = config_path()?;
    let s = std::fs::read_to_string(&p).ok()?;
    let v = s.parse::<toml::Value>().ok()?;
    v.get(key)
        .and_then(|x| x.as_str())
        .map(|x| x.to_string())
        .filter(|x| !x.trim().is_empty())
}

/// 命令：测试与 DeepSeek API 的连通性。
/// 优先使用传入的 api_key/base_url（用户刚输入但未保存），否则回退环境变量/配置文件；
/// 通过 GET {base}/models 验证鉴权是否有效。
#[tauri::command]
fn test_connection(api_key: Option<String>, base_url: Option<String>) -> Result<String, String> {
    // 解析 API Key：入参 → 环境变量 → 配置文件
    let key = api_key
        .map(|k| k.trim().to_string())
        .filter(|k| !k.is_empty())
        .or_else(|| std::env::var("DEEPSEEK_API_KEY").ok().filter(|k| !k.trim().is_empty()))
        .or_else(|| config_str("api_key"))
        .ok_or_else(|| "未配置 API Key".to_string())?;

    // 解析 Base URL：入参 → 配置文件 → 默认官方域名
    let base = base_url
        .map(|b| b.trim().to_string())
        .filter(|b| !b.is_empty())
        .or_else(|| config_str("base_url"))
        .unwrap_or_else(|| "https://api.deepseek.com".to_string());
    let url = format!("{}/models", base.trim_end_matches('/'));

    // 打 /models 端点（OpenAI 兼容），15s 超时
    let resp = ureq::get(&url)
        .timeout(std::time::Duration::from_secs(15))
        .set("Authorization", &format!("Bearer {key}"))
        .call();

    match resp {
        Ok(r) => Ok(format!("连接成功（HTTP {}）", r.status())),
        Err(ureq::Error::Status(401, _)) => {
            Err("连接失败：HTTP 401，API Key 无效或已过期".to_string())
        }
        Err(ureq::Error::Status(code, _)) => {
            Err(format!("连接失败：HTTP {code}，请检查 Base URL 与 Key"))
        }
        Err(e) => Err(format!("连接失败：{e}")),
    }
}

// ===================== 模型配置档案（多供应商管理）=====================

/// 单个模型配置档案
#[derive(serde::Serialize, serde::Deserialize, Clone, Default)]
struct Profile {
    /// 档案唯一 id
    id: String,
    /// 配置名称（用户自定义，如 "DeepSeek 主号"）
    name: String,
    /// 服务商标识（deepseek / ollama / openai …）
    #[serde(default)]
    provider: String,
    /// API Base URL
    #[serde(default)]
    base_url: String,
    /// 模型名称
    #[serde(default)]
    model: String,
    /// API Key（明文存于本地 JSON，与 config.toml 同等本地信任级别）
    #[serde(default)]
    api_key: String,
}

/// 档案集合文档
#[derive(serde::Serialize, serde::Deserialize, Default)]
struct ProfilesDoc {
    /// 当前「使用中」档案 id
    #[serde(default)]
    active_id: String,
    /// 全部档案
    #[serde(default)]
    profiles: Vec<Profile>,
}

/// 档案存储文件路径：~/.deepseek/gui_profiles.json
fn profiles_path() -> Option<PathBuf> {
    let home = std::env::var("USERPROFILE")
        .ok()
        .or_else(|| std::env::var("HOME").ok())?;
    Some(
        PathBuf::from(home)
            .join(".deepseek")
            .join("gui_profiles.json"),
    )
}

/// 读取档案文档（不存在或损坏则返回空文档）
fn load_profiles() -> ProfilesDoc {
    profiles_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str::<ProfilesDoc>(&s).ok())
        .unwrap_or_default()
}

/// 写入档案文档
fn store_profiles(doc: &ProfilesDoc) -> Result<(), String> {
    let p = profiles_path().ok_or_else(|| "无法定位配置目录".to_string())?;
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let s = serde_json::to_string_pretty(doc).map_err(|e| e.to_string())?;
    std::fs::write(&p, s).map_err(|e| e.to_string())?;
    Ok(())
}

/// 命令：列出全部档案（Key 仅返回是否存在与掩码，不回传明文）
#[tauri::command]
fn list_profiles() -> serde_json::Value {
    let doc = load_profiles();
    let profiles: Vec<serde_json::Value> = doc
        .profiles
        .iter()
        .map(|p| {
            let present = !p.api_key.trim().is_empty();
            serde_json::json!({
                "id": p.id,
                "name": p.name,
                "provider": p.provider,
                "base_url": p.base_url,
                "model": p.model,
                "key_present": present,
                "key_masked": if present { "********" } else { "" },
            })
        })
        .collect();
    serde_json::json!({ "active_id": doc.active_id, "profiles": profiles })
}

/// 命令：新增或更新档案。
/// 传入 id 为空 → 新增并返回新 id；否则按 id 更新。
/// api_key 留空表示「保持原 Key 不变」（更新场景）。新增且无 active 时自动设为使用中。
#[tauri::command]
fn upsert_profile(profile: serde_json::Value) -> Result<String, String> {
    let mut doc = load_profiles();
    let id_in = profile
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let name = profile
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if name.is_empty() {
        return Err("配置名称不能为空".to_string());
    }
    let provider = profile
        .get("provider")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let base_url = profile
        .get("base_url")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let model = profile
        .get("model")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let key_in = profile
        .get("api_key")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();

    if id_in.is_empty() {
        // 新增：用毫秒时间戳生成 id
        let id = format!(
            "p{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0)
        );
        if key_in.is_empty() {
            return Err("新增配置必须填写 API Key".to_string());
        }
        doc.profiles.push(Profile {
            id: id.clone(),
            name,
            provider,
            base_url,
            model,
            api_key: key_in,
        });
        // 首个档案自动设为使用中
        if doc.active_id.trim().is_empty() {
            doc.active_id = id.clone();
        }
        store_profiles(&doc)?;
        Ok(id)
    } else {
        // 更新：定位现有档案
        let p = doc
            .profiles
            .iter_mut()
            .find(|p| p.id == id_in)
            .ok_or_else(|| "未找到要更新的配置".to_string())?;
        p.name = name;
        p.provider = provider;
        p.base_url = base_url;
        p.model = model;
        // Key 留空则保留原值
        if !key_in.is_empty() {
            p.api_key = key_in;
        }
        store_profiles(&doc)?;
        Ok(id_in)
    }
}

/// 命令：删除档案。若删的是使用中档案，则清空 active（或落到第一个）。
#[tauri::command]
fn delete_profile(id: String) -> Result<(), String> {
    let mut doc = load_profiles();
    doc.profiles.retain(|p| p.id != id);
    if doc.active_id == id {
        doc.active_id = doc.profiles.first().map(|p| p.id.clone()).unwrap_or_default();
    }
    store_profiles(&doc)?;
    Ok(())
}

/// 命令：将某档案设为「使用中」——把其 Key/BaseURL/模型/服务商写入
/// config.toml 后重启后端使其生效。
#[tauri::command]
fn activate_profile(state: State<Backend>, id: String) -> Result<(), String> {
    let mut doc = load_profiles();
    let profile = doc
        .profiles
        .iter()
        .find(|p| p.id == id)
        .cloned()
        .ok_or_else(|| "未找到该配置".to_string())?;

    // 写入 config.toml（保留其他键）
    let cfg = config_path().ok_or_else(|| "无法定位配置目录".to_string())?;
    if let Some(dir) = cfg.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let mut toml_doc: toml::Value = std::fs::read_to_string(&cfg)
        .ok()
        .and_then(|s| s.parse::<toml::Value>().ok())
        .unwrap_or_else(|| toml::Value::Table(Default::default()));
    if let toml::Value::Table(ref mut t) = toml_doc {
        t.insert(
            "api_key".to_string(),
            toml::Value::String(profile.api_key.clone()),
        );
        // 空串视为清除，让后端用默认
        set_or_remove(t, "base_url", &profile.base_url);
        set_or_remove(t, "provider", &profile.provider);
        set_or_remove(t, "default_text_model", &profile.model);
    }
    let out = toml::to_string_pretty(&toml_doc).map_err(|e| e.to_string())?;
    std::fs::write(&cfg, out).map_err(|e| e.to_string())?;

    // 标记使用中
    doc.active_id = id;
    store_profiles(&doc)?;

    // 重启后端
    restart_backend_inner(&state)
}

/// 辅助：非空则写入键，空串则删除键
fn set_or_remove(t: &mut toml::value::Table, key: &str, val: &str) {
    let v = val.trim();
    if v.is_empty() {
        t.remove(key);
    } else {
        t.insert(key.to_string(), toml::Value::String(v.to_string()));
    }
}

/// 重启后端的内部实现（供命令复用）。重启时沿用当前工作目录设置。
fn restart_backend_inner(state: &State<Backend>) -> Result<(), String> {
    // 先取工作目录（独立锁，避免与 child 锁交叉持有）
    let workspace = state.workspace.lock().ok().and_then(|w| w.clone());
    let mut guard = state.child.lock().map_err(|e| e.to_string())?;
    if let Some(mut c) = guard.take() {
        let _ = c.kill();
        let _ = c.wait();
    }
    std::thread::sleep(std::time::Duration::from_millis(900));
    *guard = spawn_backend(&state.token, workspace.as_deref());
    if guard.is_none() {
        return Err("后端启动失败：未找到 deepseek-tui 可执行文件".to_string());
    }
    Ok(())
}

/// 命令：设置后端工作目录（agent 文件读写根目录）并重启后端使其生效。
/// 前端在「打开文件夹」或启动时携带记忆的根目录调用。
#[tauri::command]
fn set_workspace(state: State<Backend>, path: String) -> Result<(), String> {
    {
        let mut w = state.workspace.lock().map_err(|e| e.to_string())?;
        *w = if path.trim().is_empty() {
            None
        } else {
            Some(path.trim().to_string())
        };
    }
    restart_backend_inner(&state)
}

/// 命令：重启后端，使新写入的配置生效
#[tauri::command]
fn restart_backend(state: State<Backend>) -> Result<(), String> {
    restart_backend_inner(&state)
}

// ===================== LSP 编辑器补全（stdio 桥 → CodeMirror lsp-client）=====================

/// 命令：为 workspace+文件 确保 LSP 会话已启动（按语言复用 server 进程）
#[tauri::command]
async fn lsp_start_session(
    app: AppHandle,
    state: State<'_, LspBridge>,
    workspace: String,
    file_path: String,
) -> Result<lsp::LspSessionInfo, String> {
    state.ensure_session(&app, &workspace, &file_path).await
}

/// 命令：向前端 Transport 转发 JSON-RPC（纯 JSON 字符串，无 Content-Length 头）
#[tauri::command]
fn lsp_send(state: State<'_, LspBridge>, session_id: String, message: String) -> Result<(), String> {
    state.send(&session_id, &message)
}

/// 命令：关闭 LSP 会话并终止子进程
#[tauri::command]
fn lsp_stop_session(state: State<'_, LspBridge>, session_id: String) -> Result<(), String> {
    state.stop_session(&session_id)
}

/// 截取字符串末尾若干字符（按 Unicode 标量，避免切断多字节字符）
fn tail_chars(s: &str, max: usize) -> String {
    s.chars()
        .rev()
        .take(max)
        .collect::<String>()
        .chars()
        .rev()
        .collect()
}

/// 截取字符串开头若干字符
fn head_chars(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

/// 清理模型输出：去掉 markdown 围栏与首尾空白
fn sanitize_inline_completion(raw: &str) -> String {
    let mut t = raw.trim().to_string();
    if t.starts_with("```") {
        if let Some(rest) = t.strip_prefix("```") {
            let rest = rest.trim_start();
            if let Some(i) = rest.find('\n') {
                t = rest[i + 1..].to_string();
            } else {
                t = rest.to_string();
            }
        }
        if let Some(end) = t.rfind("```") {
            t = t[..end].to_string();
        }
    }
    t.trim_end().to_string()
}

/// 命令：Cursor Tab 风格 AI 内联补全（chat completions，非流式）
#[tauri::command]
fn tab_complete(
    file_path: String,
    prefix: String,
    suffix: String,
    language_id: Option<String>,
    auto_import: Option<bool>,
) -> Result<String, String> {
    let key = std::env::var("DEEPSEEK_API_KEY")
        .ok()
        .filter(|k| !k.trim().is_empty())
        .or_else(|| config_str("api_key"))
        .ok_or_else(|| "未配置 API Key".to_string())?;

    let base = config_str("base_url").unwrap_or_else(|| "https://api.deepseek.com".to_string());
    let base = base.trim_end_matches('/').to_string();
    let url = format!("{base}/chat/completions");

    let model = config_str("default_text_model").unwrap_or_else(|| "deepseek-v4-flash".to_string());
    let lang = language_id.unwrap_or_else(|| "plaintext".to_string());

    let prefix_tail = tail_chars(&prefix, 3500);
    let suffix_head = head_chars(&suffix, 1500);

    // 基础系统提示：纯插入式补全
    let mut system = String::from(
        "You are an inline code completion engine (like GitHub Copilot or Cursor Tab). \
Output ONLY the exact text to insert at the cursor between PREFIX and SUFFIX. \
No markdown fences, no quotes, no explanation. Match indentation, naming, and style of the file.",
    );
    // 自动 import：开启时允许补全在开头补上缺失的 import 语句
    if auto_import.unwrap_or(false) {
        system.push_str(
            " If the inserted code references a symbol that is not yet imported in the PREFIX, \
prepend the minimal required import statement(s) on their own line(s) before the completion, \
using the language's conventional import syntax and matching existing import style.",
        );
    }

    let user = format!(
        "File: {file_path}\nLanguage: {lang}\n\n\
PREFIX (ends at cursor):\n```\n{prefix_tail}\n```\n\n\
SUFFIX (starts after cursor):\n```\n{suffix_head}\n```"
    );

    let body = serde_json::json!({
        "model": model,
        "messages": [
            {"role": "system", "content": system},
            {"role": "user", "content": user}
        ],
        "max_tokens": 160,
        "temperature": 0.1,
        "stream": false
    });

    let body_str = serde_json::to_string(&body).map_err(|e| format!("序列化请求失败：{e}"))?;

    let resp = ureq::post(&url)
        .timeout(std::time::Duration::from_secs(12))
        .set("Authorization", &format!("Bearer {key}"))
        .set("Content-Type", "application/json")
        .send_string(&body_str);

    match resp {
        Ok(r) => {
            let status = r.status();
            let text = r.into_string().map_err(|e| format!("读取响应失败：{e}"))?;
            if status >= 400 {
                return Err(format!("Tab 补全失败：HTTP {status} — {text}"));
            }
            let v: serde_json::Value =
                serde_json::from_str(&text).map_err(|e| format!("解析 JSON 失败：{e}"))?;
            let content = v
                .pointer("/choices/0/message/content")
                .and_then(|x| x.as_str())
                .unwrap_or("");
            Ok(sanitize_inline_completion(content))
        }
        Err(ureq::Error::Status(code, r)) => {
            let detail = r.into_string().unwrap_or_default();
            Err(format!("Tab 补全失败：HTTP {code} — {detail}"))
        }
        Err(e) => Err(format!("Tab 补全请求失败：{e}")),
    }
}

// ===================== MCP / Hooks / Network 配置桥接 =====================

#[tauri::command]
fn get_mcp_config() -> Result<serde_json::Value, String> {
    read_mcp_config()
}

#[tauri::command]
fn save_mcp_config_cmd(doc: serde_json::Value) -> Result<(), String> {
    save_mcp_config(doc)
}

#[tauri::command]
fn init_mcp_config_cmd(force: bool) -> Result<serde_json::Value, String> {
    init_mcp_config(force)
}

#[tauri::command]
fn get_hooks_config() -> Result<serde_json::Value, String> {
    read_hooks_config()
}

#[tauri::command]
fn save_hooks_config_cmd(enabled: bool, hooks: serde_json::Value) -> Result<(), String> {
    save_hooks_config(enabled, hooks)
}

#[tauri::command]
fn get_network_config() -> Result<serde_json::Value, String> {
    read_network_config()
}

#[tauri::command]
fn save_network_config_cmd(
    default: String,
    allow: Vec<String>,
    deny: Vec<String>,
    audit: bool,
) -> Result<(), String> {
    save_network_config(&default, &allow, &deny, audit)
}

#[tauri::command]
fn get_subagent_state(workspace: String) -> Result<serde_json::Value, String> {
    read_subagent_state(&workspace)
}

// ===================== Memory / Note / Anchor 配置桥接 =====================

/// 命令：读取全局用户记忆内容
#[tauri::command]
fn get_memory() -> Result<serde_json::Value, String> {
    read_memory()
}

/// 命令：保存全局用户记忆内容（整文件覆盖）
#[tauri::command]
fn save_memory_cmd(content: String) -> Result<(), String> {
    save_memory(&content)
}

/// 命令：读取工作区笔记条目
#[tauri::command]
fn get_notes(workspace: String) -> Result<serde_json::Value, String> {
    read_notes(&workspace)
}

/// 命令：保存工作区笔记条目（整列表覆盖）
#[tauri::command]
fn save_notes_cmd(workspace: String, items: Vec<String>) -> Result<(), String> {
    save_notes(&workspace, &items)
}

/// 命令：读取工作区锚点条目
#[tauri::command]
fn get_anchors(workspace: String) -> Result<serde_json::Value, String> {
    read_anchors(&workspace)
}

/// 命令：保存工作区锚点条目（整列表覆盖）
#[tauri::command]
fn save_anchors_cmd(workspace: String, items: Vec<String>) -> Result<(), String> {
    save_anchors(&workspace, &items)
}

// ===================== 工作区信任目录列表桥接 =====================

/// 命令：读取某工作区的信任路径列表
#[tauri::command]
fn get_trust(workspace: String) -> Result<serde_json::Value, String> {
    read_trust(&workspace)
}

/// 命令：新增信任路径，返回实际存储的规范化路径
#[tauri::command]
fn add_trust_cmd(workspace: String, path: String) -> Result<String, String> {
    add_trust(&workspace, &path)
}

/// 命令：移除信任路径，返回是否实际移除
#[tauri::command]
fn remove_trust_cmd(workspace: String, path: String) -> Result<bool, String> {
    remove_trust(&workspace, &path)
}

fn main() {
    // 本地开发固定 token：后端默认随机生成 token，这里固定为已知值，
    // 前端通过 get_runtime_token 命令获取，实现开箱即用。
    let token =
        std::env::var("DEEPSEEK_RUNTIME_TOKEN").unwrap_or_else(|_| "dev-local-token".to_string());

    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            get_settings,
            get_runtime_token,
            save_api_key,
            get_config,
            save_config,
            restart_backend,
            pick_folder,
            pick_file,
            list_dir,
            read_file,
            write_file,
            delete_path,
            create_file,
            create_dir,
            rename_path,
            git_diff,
            save_attachment,
            test_connection,
            list_profiles,
            upsert_profile,
            delete_profile,
            activate_profile,
            set_workspace,
            lsp_start_session,
            lsp_send,
            lsp_stop_session,
            tab_complete,
            get_mcp_config,
            save_mcp_config_cmd,
            init_mcp_config_cmd,
            get_hooks_config,
            save_hooks_config_cmd,
            get_network_config,
            save_network_config_cmd,
            get_subagent_state,
            get_memory,
            save_memory_cmd,
            get_notes,
            save_notes_cmd,
            get_anchors,
            save_anchors_cmd,
            get_trust,
            add_trust_cmd,
            remove_trust_cmd,
            pty_spawn,
            pty_write,
            pty_resize,
            pty_close
        ])
        .setup({
            let token = token.clone();
            move |app| {
                // 启动时尚未选择工作目录，用默认（程序目录）；前端会在挂载时
                // 携带记忆的根目录调用 set_workspace 重启到正确目录。
                let child = spawn_backend(&token, None);
                app.manage(LspBridge::default());
                app.manage(PtyBridge::default());
                app.manage(Backend {
                    token: token.clone(),
                    child: Mutex::new(child),
                    workspace: Mutex::new(None),
                });
                Ok(())
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app_handle, event| {
            if let RunEvent::Exit = event {
                if let Some(state) = app_handle.try_state::<PtyBridge>() {
                    state.shutdown_all();
                }
                if let Some(state) = app_handle.try_state::<LspBridge>() {
                    state.shutdown_all();
                }
                if let Some(state) = app_handle.try_state::<Backend>() {
                    if let Ok(mut guard) = state.child.lock() {
                        if let Some(mut child) = guard.take() {
                            let _ = child.kill();
                        }
                    }
                }
            }
        });
}
