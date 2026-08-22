//! Legacy DeepSeek-scoped prompt complexity classifier.
//!
//! This pure scorer is retained for API compatibility, but the consolidated
//! CLI dispatcher must not use it to resolve provider-neutral `model = "auto"`:
//! doing so fabricates DeepSeek model ids for every active provider. The TUI's
//! provider-aware router owns runtime auto selection. Callers may use this
//! helper only when their candidate pair is explicitly the DeepSeek pair:
//!
//! - **`deepseek-v4-pro`** — complex tasks (debugging, refactoring, design,
//!   security review, multi-file changes, code generation, …).
//! - **`deepseek-v4-flash`** — simple tasks (lookups, formatting, small edits,
//!   translation, Q&A, …).
//!
//! This is a pure rule-based classifier. It lives in the config crate because
//! the resolved model name is a config-level concern; the route resolver never
//! sees the `"auto"` sentinel or the prompt text.

/// The resolved model name for the pro tier.
pub const PRO_MODEL: &str = "deepseek-v4-pro";

/// The resolved model name for the flash tier.
pub const FLASH_MODEL: &str = "deepseek-v4-flash";

/// The threshold score above which a task is classified as complex (pro).
/// Score ≥ 2 → pro, else → flash.
const PRO_THRESHOLD: i32 = 2;

/// Strong indicators of a complex task. Each match adds +3.
const COMPLEX_STRONG: &[&str] = &[
    // Debugging & fixing
    "debug",
    "bug",
    "fix",
    "error",
    "crash",
    "异常",
    "错误",
    "调试",
    "故障",
    "排查",
    "root cause",
    // Architecture & design
    "refactor",
    "重构",
    "architecture",
    "架构",
    "design pattern",
    "系统设计",
    "高并发",
    "分布式",
    "microservice",
    // Security
    "security",
    "安全",
    "vulnerability",
    "漏洞",
    "渗透",
    "exploit",
    // Code generation
    "implement",
    "实现",
    "generate",
    "生成",
    "create",
    "创建",
    "build",
    "构建",
    "开发",
    "prototype",
    // Complex analysis
    "analyze",
    "分析",
    "review",
    "审查",
    "audit",
    "审计",
    "optimize",
    "优化",
    "migrate",
    "迁移",
    // Multi-file / large scale
    "multi-file",
    "multiple files",
    "多个文件",
    "整个项目",
    "full project",
    "重构整个",
    "large scale",
    // Testing
    "unit test",
    "integration test",
    "e2e test",
    "测试用例",
    "test suite",
    "coverage",
    // Complex logic
    "algorithm",
    "算法",
    "状态机",
    "state machine",
    "concurrent",
    "并行",
    "异步",
    "async",
    // Documentation / PRD
    "architecture document",
    "设计文档",
    "技术方案",
    "prd",
];

/// Medium-strength indicators. Each match adds +1.
const COMPLEX_MEDIUM: &[&str] = &[
    "change",
    "修改",
    "update",
    "更新",
    "add",
    "添加",
    "新增",
    "feature",
    "功能",
    "improve",
    "改进",
    "enhance",
    "config",
    "配置",
    "setup",
    "设置",
    "deploy",
    "部署",
    "ci/cd",
    "pipeline",
    "script",
    "脚本",
    "tool",
    "工具",
    "api",
    "interface",
    "接口",
    "endpoint",
    "database",
    "数据库",
    "schema",
    "query",
    "document",
    "文档",
    "readme",
];

/// Simple-task indicators. Each match subtracts -1.
const SIMPLE: &[&str] = &[
    "find",
    "查找",
    "search",
    "搜索",
    "look up",
    "查询",
    "what is",
    "什么是",
    "explain",
    "解释",
    "tell me",
    "告诉我",
    "how to",
    "如何",
    "format",
    "格式化",
    "pretty",
    "list",
    "列出",
    "show",
    "显示",
    "print",
    "rename",
    "重命名",
    "move",
    "移动",
    "copy",
    "复制",
    "delete",
    "删除",
    "remove",
    "typo",
    "拼写",
    "spelling",
    "grammar",
    "quick",
    "快速",
    "simple",
    "简单",
    "hello world",
    "demo",
    "example",
    "示例",
    "translate",
    "翻译",
    "convert",
    "转换",
    "short",
    "简短",
    "brief",
    "简要",
];

/// Classify a prompt for the legacy DeepSeek candidate pair.
///
/// Uses a simple scoring system:
/// - Strong complex keyword: +3
/// - Medium complex keyword: +1
/// - Simple keyword: -1
/// - Prompt length > 500 chars: +2, > 200 chars: +1
/// - Contains code fence or backtick: +1
/// - Contains a file path: +1
/// - Multi-line (> 5 newlines): +1
///
/// Total ≥ 2 → `PRO_MODEL`, else → `FLASH_MODEL`.
#[must_use]
pub fn classify(prompt: &str) -> &'static str {
    if score(prompt) >= PRO_THRESHOLD {
        PRO_MODEL
    } else {
        FLASH_MODEL
    }
}

/// Compute the raw complexity score for a prompt.
#[must_use]
pub fn score(prompt: &str) -> i32 {
    let lower = prompt.to_ascii_lowercase();
    let mut score = 0i32;

    // Strong complex keywords: +3 (first match only to avoid overcounting)
    if COMPLEX_STRONG.iter().any(|kw| lower.contains(kw)) {
        score += 3;
    }

    // Medium complex keywords: +1 each
    for kw in COMPLEX_MEDIUM {
        if lower.contains(kw) {
            score += 1;
        }
    }

    // Simple keywords: -1 each
    for kw in SIMPLE {
        if lower.contains(kw) {
            score -= 1;
        }
    }

    // Length factor: long prompts tend to be more complex.
    //
    // Counted in characters, not bytes, as the doc comment above states. Half
    // the keyword lists here are Chinese, so CJK input is a first-class case —
    // and every CJK character is three UTF-8 bytes, which made `prompt.len()`
    // award the long-prompt bonus at a third of the documented length. A
    // 200-character Chinese prompt scored +2 (600 bytes) and classified as
    // complex, where the same-length English prompt scored 0.
    let len = prompt.chars().count();
    if len > 500 {
        score += 2;
    } else if len > 200 {
        score += 1;
    }

    // Code fence or backtick: actual coding task
    if prompt.contains("```") || prompt.contains('`') {
        score += 1;
    }

    // File path pattern: e.g. /path/to/file.rs or C:\path
    // Simple heuristic: path-like sequences contain / or \ and .
    if (prompt.contains('/') || prompt.contains('\\')) && prompt.contains('.') {
        score += 1;
    }

    // Multi-line: more lines = more context
    if prompt.chars().filter(|&c| c == '\n').count() > 5 {
        score += 1;
    }

    score
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_debug_task_uses_pro() {
        assert_eq!(classify("帮我调试这个bug，程序崩溃了"), PRO_MODEL);
    }

    #[test]
    fn test_refactor_task_uses_pro() {
        assert_eq!(
            classify("refactor the user module with a new architecture"),
            PRO_MODEL
        );
    }

    #[test]
    fn test_security_review_uses_pro() {
        assert_eq!(
            classify("review this code for security vulnerabilities"),
            PRO_MODEL
        );
    }

    #[test]
    fn test_simple_lookup_uses_flash() {
        assert_eq!(classify("查找昨天的日志文件"), FLASH_MODEL);
    }

    #[test]
    fn test_translation_uses_flash() {
        assert_eq!(classify("translate this to Chinese"), FLASH_MODEL);
    }

    #[test]
    fn test_formatting_uses_flash() {
        assert_eq!(classify("format this code"), FLASH_MODEL);
    }

    #[test]
    fn test_long_prompt_gets_bonus() {
        let long = "a".repeat(300);
        // No keywords, long prompt gives +1, total = 1 < 2 → flash
        assert_eq!(classify(&long), FLASH_MODEL);
    }

    #[test]
    fn test_very_long_prompt_gets_more_bonus() {
        let long = "a".repeat(600);
        // No keywords, very long prompt gives +2, total = 2 → pro
        assert_eq!(classify(&long), PRO_MODEL);
    }

    #[test]
    fn test_code_block_gets_bonus() {
        // Code block without keywords, +1, total = 1 < 2 → flash
        assert_eq!(classify("```\nhello\n```"), FLASH_MODEL);
    }

    #[test]
    fn test_mixed_keywords_pro_wins() {
        // "refactor" is strong (+3), "explain" is simple (-1), total = 2 → pro
        assert_eq!(classify("refactor and explain the code"), PRO_MODEL);
    }

    #[test]
    fn test_implement_task_uses_pro() {
        assert_eq!(
            classify("implement a new feature for the user module"),
            PRO_MODEL
        );
    }

    #[test]
    fn test_quick_question_uses_flash() {
        assert_eq!(classify("what is the capital of France?"), FLASH_MODEL);
    }

    #[test]
    fn length_bonus_counts_characters_not_utf8_bytes() {
        // Keyword-free prompts of identical *length* must score identically
        // regardless of script. "啊" is three UTF-8 bytes, so a byte-counted
        // length factor gave the Chinese prompt a bonus the English one did
        // not earn — and at 200 characters it flipped the classification.
        let english = "a".repeat(200);
        let chinese = "啊".repeat(200);
        assert_eq!(score(&chinese), score(&english));
        assert_eq!(classify(&chinese), FLASH_MODEL);

        let english_long = "a".repeat(600);
        let chinese_long = "啊".repeat(600);
        assert_eq!(score(&chinese_long), score(&english_long));
        assert_eq!(classify(&chinese_long), PRO_MODEL);
    }

    #[test]
    fn test_score_never_negative() {
        // Even for very simple queries, score should be predictable
        let s = score("hello world");
        assert!(s >= -10); // sanity check
    }
}
