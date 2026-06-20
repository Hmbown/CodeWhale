//! 语言检测与默认 LSP 可执行文件映射（与 deepseek-tui registry 对齐）

use std::path::Path;

/// GUI 编辑器支持 LSP 的语言
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Language {
    Rust,
    Go,
    Python,
    TypeScript,
    JavaScript,
    C,
    Cpp,
    Java,
    Other,
}

impl Language {
    /// LSP textDocument.languageId
    pub fn language_id(self) -> &'static str {
        match self {
            Language::Rust => "rust",
            Language::Go => "go",
            Language::Python => "python",
            Language::TypeScript => "typescript",
            Language::JavaScript => "javascript",
            Language::C => "c",
            Language::Cpp => "cpp",
            Language::Java => "java",
            Language::Other => "plaintext",
        }
    }

    /// 会话键后缀
    pub fn session_key(self) -> &'static str {
        match self {
            Language::Rust => "rust",
            Language::Go => "go",
            Language::Python => "python",
            Language::TypeScript => "typescript",
            Language::JavaScript => "javascript",
            Language::C => "c",
            Language::Cpp => "cpp",
            Language::Java => "java",
            Language::Other => "other",
        }
    }
}

/// 从文件路径推断语言
pub fn detect_language(path: &Path) -> Language {
    let ext = match path.extension().and_then(|e| e.to_str()) {
        Some(ext) => ext.to_ascii_lowercase(),
        None => return Language::Other,
    };
    match ext.as_str() {
        "rs" => Language::Rust,
        "go" => Language::Go,
        "py" | "pyi" => Language::Python,
        "ts" | "tsx" => Language::TypeScript,
        "js" | "jsx" | "mjs" | "cjs" => Language::JavaScript,
        "c" | "h" => Language::C,
        "cpp" | "cc" | "cxx" | "hpp" | "hxx" | "hh" => Language::Cpp,
        "java" => Language::Java,
        _ => Language::Other,
    }
}

/// 默认 LSP 启动命令；None 表示该语言无内置 server
pub fn server_for(lang: Language) -> Option<(&'static str, &'static [&'static str])> {
    match lang {
        Language::Rust => Some(("rust-analyzer", &[])),
        Language::Go => Some(("gopls", &["serve"])),
        Language::Python => Some(("pyright-langserver", &["--stdio"])),
        Language::TypeScript | Language::JavaScript => {
            Some(("typescript-language-server", &["--stdio"]))
        }
        Language::C | Language::Cpp => Some(("clangd", &[])),
        // jdtls 启动参数因环境差异大，MVP 暂不默认
        Language::Java => None,
        Language::Other => None,
    }
}
