//! 读写 ~/.deepseek 下的 MCP / Hooks / Network 配置（供 GUI 设置页使用）

use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// 解析 DeepSeek 配置主目录
pub fn deepseek_home() -> Option<PathBuf> {
    let home = std::env::var("USERPROFILE")
        .ok()
        .or_else(|| std::env::var("HOME").ok())?;
    Some(PathBuf::from(home).join(".deepseek"))
}

/// config.toml 路径
pub fn config_toml_path() -> Option<PathBuf> {
    deepseek_home().map(|h| h.join("config.toml"))
}

/// 从 config.toml 读取 mcp.json 路径（缺省 ~/.deepseek/mcp.json）
pub fn mcp_json_path() -> Option<PathBuf> {
    if let Some(p) = config_toml_path() {
        if let Ok(s) = std::fs::read_to_string(&p) {
            if let Ok(v) = s.parse::<toml::Value>() {
                if let Some(custom) = v.get("mcp_config_path").and_then(|x| x.as_str()) {
                    if !custom.trim().is_empty() {
                        return Some(PathBuf::from(custom));
                    }
                }
            }
        }
    }
    deepseek_home().map(|h| h.join("mcp.json"))
}

/// 读取 MCP 配置 JSON
pub fn read_mcp_config() -> Result<Value, String> {
    let path = mcp_json_path().ok_or_else(|| "无法定位 MCP 配置路径".to_string())?;
    if !path.exists() {
        return Ok(json!({ "path": path.to_string_lossy(), "exists": false, "servers": {} }));
    }
    let s = std::fs::read_to_string(&path).map_err(|e| format!("读取 MCP 配置失败：{e}"))?;
    let mut v: Value = serde_json::from_str(&s).map_err(|e| format!("MCP JSON 解析失败：{e}"))?;
    if let Some(obj) = v.as_object_mut() {
        obj.insert("path".to_string(), json!(path.to_string_lossy()));
        obj.insert("exists".to_string(), json!(true));
        if !obj.contains_key("servers") {
            if let Some(alt) = obj.get("mcpServers").cloned() {
                obj.insert("servers".to_string(), alt);
            }
        }
    }
    Ok(v)
}

/// 保存 MCP 配置
pub fn save_mcp_config(mut doc: Value) -> Result<(), String> {
    let path = mcp_json_path().ok_or_else(|| "无法定位 MCP 配置路径".to_string())?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    if let Some(obj) = doc.as_object_mut() {
        obj.remove("path");
        obj.remove("exists");
    }
    let out = serde_json::to_string_pretty(&doc).map_err(|e| e.to_string())?;
    std::fs::write(&path, out).map_err(|e| format!("写入 MCP 配置失败：{e}"))?;
    Ok(())
}

/// 初始化空 mcp.json
pub fn init_mcp_config(force: bool) -> Result<Value, String> {
    let path = mcp_json_path().ok_or_else(|| "无法定位 MCP 配置路径".to_string())?;
    if path.exists() && !force {
        return Err("MCP 配置已存在".to_string());
    }
    let empty = json!({
        "servers": {},
        "timeouts": { "connect_timeout": 10, "execute_timeout": 60, "read_timeout": 120 }
    });
    save_mcp_config(empty.clone())?;
    Ok(empty)
}

/// 读取 [hooks] 配置段
pub fn read_hooks_config() -> Result<Value, String> {
    let path = config_toml_path().ok_or_else(|| "无法定位 config.toml".to_string())?;
    let mut out = json!({
        "config_path": path.to_string_lossy(),
        "enabled": true,
        "hooks": []
    });
    if !path.exists() {
        return Ok(out);
    }
    let s = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let v = s.parse::<toml::Value>().map_err(|e| e.to_string())?;
    if let Some(hooks) = v.get("hooks") {
        if let Some(en) = hooks.get("enabled").and_then(|x| x.as_bool()) {
            out["enabled"] = json!(en);
        }
        if let Some(arr) = hooks.get("hooks").and_then(|x| x.as_array()) {
            let items: Vec<Value> = arr.iter().filter_map(toml_to_json).collect();
            out["hooks"] = json!(items);
        }
    }
    Ok(out)
}

/// 保存 hooks 配置
pub fn save_hooks_config(enabled: bool, hooks: Value) -> Result<(), String> {
    let path = config_toml_path().ok_or_else(|| "无法定位 config.toml".to_string())?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let mut doc: toml::Value = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| toml::Value::Table(Default::default()));
    let root = doc.as_table_mut().ok_or_else(|| "config 根节点无效".to_string())?;
    let hooks_arr = hooks
        .as_array()
        .ok_or_else(|| "hooks 必须为数组".to_string())?
        .iter()
        .filter_map(json_to_toml)
        .collect::<Vec<_>>();
    let mut hooks_table = toml::map::Map::new();
    hooks_table.insert("enabled".to_string(), toml::Value::Boolean(enabled));
    hooks_table.insert("hooks".to_string(), toml::Value::Array(hooks_arr));
    root.insert("hooks".to_string(), toml::Value::Table(hooks_table));
    let out = toml::to_string_pretty(&doc).map_err(|e| e.to_string())?;
    std::fs::write(&path, out).map_err(|e| e.to_string())?;
    Ok(())
}

/// 读取 [network] 策略
pub fn read_network_config() -> Result<Value, String> {
    let path = config_toml_path().ok_or_else(|| "无法定位 config.toml".to_string())?;
    let mut out = json!({
        "config_path": path.to_string_lossy(),
        "default": "prompt",
        "allow": [],
        "deny": [],
        "audit": true
    });
    if !path.exists() {
        return Ok(out);
    }
    let s = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let v = s.parse::<toml::Value>().map_err(|e| e.to_string())?;
    if let Some(net) = v.get("network").and_then(|x| x.as_table()) {
        if let Some(d) = net.get("default").and_then(|x| x.as_str()) {
            out["default"] = json!(d);
        }
        if let Some(a) = net.get("audit").and_then(|x| x.as_bool()) {
            out["audit"] = json!(a);
        }
        if let Some(arr) = net.get("allow").and_then(|x| x.as_array()) {
            out["allow"] = json!(arr.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>());
        }
        if let Some(arr) = net.get("deny").and_then(|x| x.as_array()) {
            out["deny"] = json!(arr.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>());
        }
    }
    Ok(out)
}

/// 保存 network 策略
pub fn save_network_config(
    default: &str,
    allow: &[String],
    deny: &[String],
    audit: bool,
) -> Result<(), String> {
    let path = config_toml_path().ok_or_else(|| "无法定位 config.toml".to_string())?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let mut doc: toml::Value = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| toml::Value::Table(Default::default()));
    let root = doc.as_table_mut().ok_or_else(|| "config 根节点无效".to_string())?;
    let mut net = toml::map::Map::new();
    net.insert("default".to_string(), toml::Value::String(default.to_string()));
    net.insert(
        "allow".to_string(),
        toml::Value::Array(allow.iter().map(|h| toml::Value::String(h.clone())).collect()),
    );
    net.insert(
        "deny".to_string(),
        toml::Value::Array(deny.iter().map(|h| toml::Value::String(h.clone())).collect()),
    );
    net.insert("audit".to_string(), toml::Value::Boolean(audit));
    root.insert("network".to_string(), toml::Value::Table(net));
    let out = toml::to_string_pretty(&doc).map_err(|e| e.to_string())?;
    std::fs::write(&path, out).map_err(|e| e.to_string())?;
    Ok(())
}

// ===================== Memory / Note / Anchor 读写 =====================
//
// 与 TUI 保持一致的存储约定：
//   - 用户记忆 memory：全局文件 ~/.deepseek/memory.md（纯 Markdown 文本）。
//   - 工作区笔记 note：<workspace>/.deepseek/notes.md，条目以 "\n---\n" 分隔。
//   - 工作区锚点 anchor：<workspace>/.deepseek/anchors.md，条目以 "\n---\n" 分隔。
// GUI 读写时复用这些路径，保证与 TUI 的 /memory、/note、/anchor 命令互通。

/// 全局用户记忆文件路径 ~/.deepseek/memory.md
fn memory_path() -> Option<PathBuf> {
    deepseek_home().map(|h| h.join("memory.md"))
}

/// 工作区笔记文件路径 <workspace>/.deepseek/notes.md
fn notes_path(workspace: &str) -> PathBuf {
    Path::new(workspace).join(".deepseek").join("notes.md")
}

/// 工作区锚点文件路径 <workspace>/.deepseek/anchors.md
fn anchors_path(workspace: &str) -> PathBuf {
    Path::new(workspace).join(".deepseek").join("anchors.md")
}

/// 将以 "\n---\n" 分隔的文本拆分为条目列表（去除首尾空白并过滤空条目）
fn split_entries(content: &str) -> Vec<String> {
    content
        .split("\n---\n")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// 将条目列表合并为以 "\n---\n" 分隔的存储文本
fn join_entries(items: &[String]) -> String {
    items
        .iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("\n---\n")
}

/// 读取全局用户记忆内容
pub fn read_memory() -> Result<Value, String> {
    let path = memory_path().ok_or_else(|| "无法定位记忆文件路径".to_string())?;
    if !path.exists() {
        return Ok(json!({ "path": path.to_string_lossy(), "exists": false, "content": "" }));
    }
    let content = std::fs::read_to_string(&path).map_err(|e| format!("读取记忆文件失败：{e}"))?;
    Ok(json!({ "path": path.to_string_lossy(), "exists": true, "content": content }))
}

/// 保存全局用户记忆内容（整文件覆盖写入）
pub fn save_memory(content: &str) -> Result<(), String> {
    let path = memory_path().ok_or_else(|| "无法定位记忆文件路径".to_string())?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(&path, content).map_err(|e| format!("写入记忆文件失败：{e}"))
}

/// 读取工作区笔记条目列表
pub fn read_notes(workspace: &str) -> Result<Value, String> {
    let path = notes_path(workspace);
    if !path.exists() {
        return Ok(json!({ "path": path.to_string_lossy(), "exists": false, "items": [] }));
    }
    let s = std::fs::read_to_string(&path).map_err(|e| format!("读取笔记失败：{e}"))?;
    Ok(json!({ "path": path.to_string_lossy(), "exists": true, "items": split_entries(&s) }))
}

/// 保存工作区笔记条目列表（整文件覆盖写入）
pub fn save_notes(workspace: &str, items: &[String]) -> Result<(), String> {
    let path = notes_path(workspace);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(&path, join_entries(items)).map_err(|e| format!("写入笔记失败：{e}"))
}

/// 读取工作区锚点条目列表
pub fn read_anchors(workspace: &str) -> Result<Value, String> {
    let path = anchors_path(workspace);
    if !path.exists() {
        return Ok(json!({ "path": path.to_string_lossy(), "exists": false, "items": [] }));
    }
    let s = std::fs::read_to_string(&path).map_err(|e| format!("读取锚点失败：{e}"))?;
    Ok(json!({ "path": path.to_string_lossy(), "exists": true, "items": split_entries(&s) }))
}

/// 保存工作区锚点条目列表（整文件覆盖写入）
pub fn save_anchors(workspace: &str, items: &[String]) -> Result<(), String> {
    let path = anchors_path(workspace);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(&path, join_entries(items)).map_err(|e| format!("写入锚点失败：{e}"))
}

// ===================== 工作区信任目录列表 =====================
//
// 与 TUI workspace_trust 保持一致：存储在 ~/.deepseek/workspace-trust.json，
// 结构为 { "workspaces": { <规范化工作区路径>: [<规范化信任路径>...] } }。
// 路径统一用 std 规范化（Windows 会带 \\?\ 前缀），与 TUI 生成的键互通。

/// 信任列表文件路径 ~/.deepseek/workspace-trust.json
fn trust_file_path() -> Option<PathBuf> {
    deepseek_home().map(|h| h.join("workspace-trust.json"))
}

/// 规范化路径；失败时保留原值（与 TUI canonicalize_or_keep 一致）
fn canonicalize_or_keep(path: &str) -> String {
    let p = Path::new(path);
    p.canonicalize()
        .unwrap_or_else(|_| p.to_path_buf())
        .to_string_lossy()
        .into_owned()
}

/// 读取整个信任文件（不存在或损坏返回空对象）
fn read_trust_doc() -> Value {
    trust_file_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .unwrap_or_else(|| json!({ "workspaces": {} }))
}

/// 写入整个信任文件
fn write_trust_doc(doc: &Value) -> Result<(), String> {
    let path = trust_file_path().ok_or_else(|| "无法定位信任列表路径".to_string())?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let out = serde_json::to_string_pretty(doc).map_err(|e| e.to_string())?;
    std::fs::write(&path, out).map_err(|e| format!("写入信任列表失败：{e}"))
}

/// 读取某工作区的信任路径列表
pub fn read_trust(workspace: &str) -> Result<Value, String> {
    let key = canonicalize_or_keep(workspace);
    let doc = read_trust_doc();
    let items = doc
        .get("workspaces")
        .and_then(|w| w.get(&key))
        .and_then(|x| x.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect::<Vec<_>>())
        .unwrap_or_default();
    let file = trust_file_path()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    Ok(json!({ "path": file, "key": key, "items": items }))
}

/// 向某工作区信任列表新增路径，返回实际存储的规范化路径
pub fn add_trust(workspace: &str, path: &str) -> Result<String, String> {
    let key = canonicalize_or_keep(workspace);
    let stored = canonicalize_or_keep(path);
    let mut doc = read_trust_doc();
    let workspaces = doc
        .get_mut("workspaces")
        .and_then(|w| w.as_object_mut())
        .ok_or_else(|| "信任文件格式错误".to_string())?;
    let entry = workspaces
        .entry(key)
        .or_insert_with(|| Value::Array(vec![]));
    let arr = entry.as_array_mut().ok_or_else(|| "信任条目格式错误".to_string())?;
    if !arr.iter().any(|x| x.as_str() == Some(stored.as_str())) {
        arr.push(json!(stored));
        arr.sort_by(|a, b| a.as_str().unwrap_or("").cmp(b.as_str().unwrap_or("")));
    }
    write_trust_doc(&doc)?;
    Ok(stored)
}

/// 从某工作区信任列表移除路径，返回是否实际移除
pub fn remove_trust(workspace: &str, path: &str) -> Result<bool, String> {
    let key = canonicalize_or_keep(workspace);
    let stored = canonicalize_or_keep(path);
    let mut doc = read_trust_doc();
    let Some(workspaces) = doc.get_mut("workspaces").and_then(|w| w.as_object_mut()) else {
        return Ok(false);
    };
    let mut removed = false;
    let mut clear_key = false;
    if let Some(arr) = workspaces.get_mut(&key).and_then(|x| x.as_array_mut()) {
        let before = arr.len();
        arr.retain(|x| x.as_str() != Some(stored.as_str()));
        removed = arr.len() != before;
        clear_key = arr.is_empty();
    }
    if clear_key {
        workspaces.remove(&key);
    }
    if removed {
        write_trust_doc(&doc)?;
    }
    Ok(removed)
}

/// 读取工作区 subagents 状态文件
pub fn read_subagent_state(workspace: &str) -> Result<Value, String> {
    let p = Path::new(workspace).join(".deepseek").join("subagents.v1.json");
    if !p.exists() {
        return Ok(json!({ "path": p.to_string_lossy(), "exists": false, "raw": null }));
    }
    let s = std::fs::read_to_string(&p).map_err(|e| e.to_string())?;
    let v: Value = serde_json::from_str(&s).map_err(|e| e.to_string())?;
    Ok(json!({ "path": p.to_string_lossy(), "exists": true, "raw": v }))
}

fn toml_to_json(v: &toml::Value) -> Option<Value> {
    Some(match v {
        toml::Value::String(s) => json!(s),
        toml::Value::Integer(i) => json!(i),
        toml::Value::Float(f) => json!(f),
        toml::Value::Boolean(b) => json!(b),
        toml::Value::Array(a) => json!(a.iter().filter_map(toml_to_json).collect::<Vec<_>>()),
        toml::Value::Table(t) => {
            let mut m = serde_json::Map::new();
            for (k, val) in t {
                if let Some(j) = toml_to_json(val) {
                    m.insert(k.clone(), j);
                }
            }
            Value::Object(m)
        }
        _ => return None,
    })
}

fn json_to_toml(v: &Value) -> Option<toml::Value> {
    match v {
        Value::String(s) => Some(toml::Value::String(s.clone())),
        Value::Number(n) => n
            .as_i64()
            .map(toml::Value::Integer)
            .or_else(|| n.as_f64().map(toml::Value::Float)),
        Value::Bool(b) => Some(toml::Value::Boolean(*b)),
        Value::Array(a) => Some(toml::Value::Array(
            a.iter().filter_map(json_to_toml).collect(),
        )),
        Value::Object(o) => {
            let mut t = toml::map::Map::new();
            for (k, val) in o {
                if let Some(tv) = json_to_toml(val) {
                    t.insert(k.clone(), tv);
                }
            }
            Some(toml::Value::Table(t))
        }
        _ => None,
    }
}
