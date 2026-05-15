// IPC commands exposed to the frontend (JS/HTML side)
// All calls proxy through to the Python extension layer HTTP API

use reqwest::Method;
use serde_json::{json, Value};
use tauri::command;

const CORE_URL: &str = "http://localhost:3000";

async fn send_json(method: Method, path: &str, body: Option<Value>) -> Result<Value, String> {
    let client = reqwest::Client::new();
    let mut request = client.request(method, format!("{}{}", CORE_URL, path));
    if let Some(payload) = body {
        request = request.json(&payload);
    }
    let resp = request.send().await.map_err(|e| e.to_string())?;
    let resp = resp.error_for_status().map_err(|e| e.to_string())?;
    resp.json::<Value>().await.map_err(|e| e.to_string())
}

#[command]
pub async fn send_message(message: String) -> Result<String, String> {
    let body = send_json(Method::POST, "/v1/chat", Some(json!({ "message": message }))).await?;
    Ok(body["content"].as_str().unwrap_or("").to_string())
}

#[command]
pub async fn get_status() -> Result<Value, String> {
    send_json(Method::GET, "/v1/status", None).await
}

#[command]
pub async fn stream_message(message: String) -> Result<String, String> {
    // Simplified: returns full streamed content at once
    // TODO: replace with SSE streaming via Tauri events
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/v1/chat/stream", CORE_URL))
        .json(&serde_json::json!({ "message": message }))
        .send()
        .await
        .map_err(|e| e.to_string())?;

    let text = resp.text().await.map_err(|e| e.to_string())?;
    Ok(text)
}

#[command]
pub async fn get_capabilities() -> Result<Value, String> {
    send_json(Method::GET, "/v1/capabilities", None).await
}

#[command]
pub async fn get_browser_state() -> Result<Value, String> {
    send_json(Method::GET, "/v1/browser/state", None).await
}

#[command]
pub async fn get_browser_summary() -> Result<Value, String> {
    send_json(Method::GET, "/v1/browser/summary", None).await
}

#[command]
pub async fn start_managed_process() -> Result<Value, String> {
    send_json(Method::POST, "/control/start", None).await
}

#[command]
pub async fn restart_managed_process() -> Result<Value, String> {
    send_json(Method::POST, "/control/restart", None).await
}

#[command]
pub async fn stop_managed_process() -> Result<Value, String> {
    send_json(Method::POST, "/control/stop", None).await
}

#[command]
pub async fn navigate_browser(url: String) -> Result<Value, String> {
    send_json(Method::POST, "/v1/browser/navigate", Some(json!({ "url": url }))).await
}

#[command]
pub async fn reload_browser_console() -> Result<Value, String> {
    send_json(Method::POST, "/v1/browser/reload-console", Some(json!({}))).await
}

#[command]
pub async fn update_browser_ui(
    viewport_width: Option<u32>,
    viewport_height: Option<u32>,
    zoom: Option<f64>,
    css: Option<String>,
    js: Option<String>,
    reset_injections: Option<bool>,
) -> Result<Value, String> {
    send_json(
        Method::POST,
        "/v1/browser/ui",
        Some(json!({
            "viewport_width": viewport_width,
            "viewport_height": viewport_height,
            "zoom": zoom,
            "css": css,
            "js": js,
            "reset_injections": reset_injections.unwrap_or(false),
        })),
    )
    .await
}

#[command]
pub async fn evaluate_browser(expression: String) -> Result<Value, String> {
    send_json(
        Method::POST,
        "/v1/browser/evaluate",
        Some(json!({ "expression": expression })),
    )
    .await
}

#[command]
pub async fn screenshot_browser(path: Option<String>) -> Result<Value, String> {
    send_json(Method::POST, "/v1/browser/screenshot", Some(json!({ "path": path }))).await
}

#[command]
pub async fn inspect_browser_dom(selector: String) -> Result<Value, String> {
    send_json(Method::POST, "/v1/browser/dom", Some(json!({ "selector": selector }))).await
}

#[command]
pub async fn operate_browser_element(
    selector: String,
    action: String,
    value: Option<String>,
) -> Result<Value, String> {
    send_json(
        Method::POST,
        "/v1/browser/element",
        Some(json!({
            "selector": selector,
            "action": action,
            "value": value,
        })),
    )
    .await
}

#[command]
pub async fn run_mcp_browser_task(prompt: String) -> Result<Value, String> {
    send_json(
        Method::POST,
        "/v1/browser/mcp-task",
        Some(json!({ "prompt": prompt })),
    )
    .await
}

