// CodeMirror 语言扩展：按文件路径选择语法高亮包

import { javascript } from "@codemirror/lang-javascript";
import { html } from "@codemirror/lang-html";
import { css } from "@codemirror/lang-css";
import { json } from "@codemirror/lang-json";
import { markdown } from "@codemirror/lang-markdown";
import { python } from "@codemirror/lang-python";
import { rust } from "@codemirror/lang-rust";
import { java } from "@codemirror/lang-java";
import { cpp } from "@codemirror/lang-cpp";
import { sql } from "@codemirror/lang-sql";
import { xml } from "@codemirror/lang-xml";
import { StreamLanguage } from "@codemirror/language";
import { properties } from "@codemirror/legacy-modes/mode/properties";
import { toml } from "@codemirror/legacy-modes/mode/toml";
import { yaml } from "@codemirror/legacy-modes/mode/yaml";
import type { Extension } from "@codemirror/state";

/** Java/Spring .properties、.ini、.env 等键值配置 */
const propertiesLang = StreamLanguage.define(properties);
/** TOML 配置（Cargo.toml 等） */
const tomlLang = StreamLanguage.define(toml);
/** YAML 配置 */
const yamlLang = StreamLanguage.define(yaml);

/** 从路径解析语言 id（含 .env、application.properties 等特殊文件名） */
function languageIdFromPath(path: string | null): string {
  if (!path) return "";
  const base = path.split(/[\\/]/).pop()?.toLowerCase() ?? "";
  if (base === ".env" || base.startsWith(".env.")) return "dotenv";
  if (base.endsWith(".properties")) return "properties";
  const ext = base.includes(".") ? (base.split(".").pop() ?? "") : base;
  return ext;
}

/** 按路径返回 CodeMirror 语言扩展（无匹配时返回空，即纯文本） */
export function langExtensionsForPath(path: string | null): Extension[] {
  const id = languageIdFromPath(path);
  switch (id) {
    case "ts":
      return [javascript({ typescript: true })];
    case "tsx":
      return [javascript({ typescript: true, jsx: true })];
    case "js":
      return [javascript()];
    case "jsx":
      return [javascript({ jsx: true })];
    case "java":
      return [java()];
    case "c":
    case "h":
    case "cpp":
    case "cc":
    case "cxx":
    case "hpp":
    case "hh":
      return [cpp()];
    case "sql":
      return [sql()];
    case "xml":
    case "xhtml":
    case "svg":
    case "pom":
      return [xml()];
    case "html":
    case "htm":
      return [html()];
    case "css":
    case "scss":
    case "less":
      return [css()];
    case "json":
    case "jsonc":
      return [json()];
    case "md":
    case "mdc":
      return [markdown()];
    case "py":
      return [python()];
    case "rs":
      return [rust()];
    case "yaml":
    case "yml":
      return [yamlLang];
    case "properties":
    case "ini":
    case "dotenv":
      return [propertiesLang];
    case "toml":
      return [tomlLang];
    default:
      return [];
  }
}
