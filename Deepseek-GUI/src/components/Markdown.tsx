// Markdown 渲染组件：用于助手消息的富文本展示
// marked 解析 + highlight.js 代码高亮 + DOMPurify 清洗，防止 XSS

import { useMemo } from "react";
import { marked, Renderer } from "marked";
import hljs from "highlight.js/lib/common";
import DOMPurify from "dompurify";

// 自定义渲染器：代码块走 highlight.js
const renderer = new Renderer();
renderer.code = ({ text, lang }: { text: string; lang?: string }) => {
  let body: string;
  try {
    if (lang && hljs.getLanguage(lang)) {
      body = hljs.highlight(text, { language: lang }).value;
    } else {
      body = hljs.highlightAuto(text).value;
    }
  } catch {
    body = escapeHtml(text);
  }
  const label = lang ? `<span class="md-code-lang">${escapeHtml(lang)}</span>` : "";
  return `<pre class="md-pre">${label}<code class="hljs">${body}</code></pre>`;
};

marked.setOptions({ gfm: true, breaks: true });
marked.use({ renderer });

interface MarkdownProps {
  /** 原始 Markdown 文本 */
  text: string;
}

/** 渲染 Markdown 为安全 HTML */
export function Markdown({ text }: MarkdownProps) {
  const html = useMemo(() => {
    const raw = marked.parse(text, { async: false }) as string;
    return DOMPurify.sanitize(raw);
  }, [text]);
  return <div className="md" dangerouslySetInnerHTML={{ __html: html }} />;
}

/** HTML 转义兜底 */
function escapeHtml(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}
