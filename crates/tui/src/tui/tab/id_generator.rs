//! 统一ID生成器 - Unified ID Generator
//!
//! 提供全局唯一的ID生成功能，支持多种前缀和格式。

use chrono::Utc;
use std::sync::atomic::{AtomicU64, Ordering};

/// 全局ID生成器
pub struct IdGenerator {
    counter: AtomicU64,
}

impl IdGenerator {
    /// 生成新ID
    pub fn next(&self) -> u64 {
        self.counter.fetch_add(1, Ordering::Relaxed)
    }
}

// 全局实例
static ID_GENERATOR: IdGenerator = IdGenerator {
    counter: AtomicU64::new(1),
};

/// 生成唯一的Tab ID
pub fn generate_tab_id() -> String {
    let id = ID_GENERATOR.next();
    let timestamp = Utc::now().timestamp();
    format!("tab_{}_{}", timestamp, id)
}

/// 生成唯一的任务ID
pub fn generate_task_id() -> String {
    let id = ID_GENERATOR.next();
    let timestamp = Utc::now().timestamp();
    format!("task_{}_{}", timestamp, id)
}

/// 生成唯一的会议ID
pub fn generate_meeting_id() -> String {
    let id = ID_GENERATOR.next();
    let timestamp = Utc::now().timestamp();
    format!("mtg_{}_{}", timestamp, id)
}

/// 生成唯一的消息ID
pub fn generate_message_id() -> String {
    let id = ID_GENERATOR.next();
    let timestamp = Utc::now().timestamp_millis();
    format!("msg_{}_{}", timestamp, id)
}

/// 生成唯一的委托ID
pub fn generate_delegation_id() -> String {
    let id = ID_GENERATOR.next();
    let timestamp = Utc::now().timestamp();
    format!("dlg_{}_{}", timestamp, id)
}

/// 生成唯一的会话ID
pub fn generate_session_id() -> String {
    let id = ID_GENERATOR.next();
    let timestamp = Utc::now().timestamp();
    format!("sess_{}_{}", timestamp, id)
}