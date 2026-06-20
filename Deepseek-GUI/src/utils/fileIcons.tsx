// 文件树图标（仿 Cursor / VS Code Material 风格）：按扩展名与文件夹名返回 SVG

/** 图标通用属性 */
interface IconProps {
  className?: string;
}

/** 展开/折叠箭头（Chevron） */
export function TreeChevron({ open, className }: { open: boolean } & IconProps) {
  return (
    <svg
      viewBox="0 0 16 16"
      className={className}
      aria-hidden
      style={{ transform: open ? "rotate(90deg)" : "none", transition: "transform 0.12s ease" }}
    >
      <path
        d="M6 4l4 4-4 4"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.4"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  );
}

/** 带字母徽标的文件图标 */
function BadgeIcon({
  label,
  bg,
  fg = "#1e1e1e",
  className,
}: {
  label: string;
  bg: string;
  fg?: string;
} & IconProps) {
  return (
    <svg viewBox="0 0 16 16" className={className} aria-hidden>
      <rect x="1" y="2" width="14" height="12" rx="2" fill={bg} />
      <text
        x="8"
        y="10.5"
        textAnchor="middle"
        fontSize={label.length > 2 ? "4.5" : "6"}
        fontWeight="700"
        fill={fg}
        fontFamily="Segoe UI, system-ui, sans-serif"
      >
        {label}
      </text>
    </svg>
  );
}

/** 通用文档图标 */
function DocIcon({ color, className }: { color: string } & IconProps) {
  return (
    <svg viewBox="0 0 16 16" className={className} aria-hidden>
      <path
        d="M4 1h6l3 3v11a1 1 0 0 1-1 1H4a1 1 0 0 1-1-1V2a1 1 0 0 1 1-1z"
        fill={color}
        opacity="0.9"
      />
      <path d="M10 1v3h3" fill="none" stroke="#1e1e1e" strokeWidth="0.6" opacity="0.35" />
    </svg>
  );
}

/** 文件夹图标 */
export function FolderIcon({
  name,
  open,
  className,
}: {
  name: string;
  open: boolean;
} & IconProps) {
  const lower = name.toLowerCase();
  // 特殊目录配色（仿 VS Code 文件夹主题色）
  let fill = open ? "#dcb67a" : "#c5a332";
  if (lower === "node_modules" || lower === ".git") fill = open ? "#6b8e23" : "#5a7a1a";
  else if (lower === "src" || lower === "frontend" || lower === "backend") fill = open ? "#7eb6e0" : "#519aba";
  else if (lower.startsWith(".")) fill = open ? "#848484" : "#6e6e6e";

  return (
    <svg viewBox="0 0 16 16" className={className} aria-hidden>
      {open ? (
        <>
          <path d="M1.5 3.5h5l1.2 1.2H14v8.8H1.5V3.5z" fill={fill} />
          <path d="M1.5 3.5h5l1.2 1.2H14" fill="none" stroke="#1e1e1e" strokeWidth="0.5" opacity="0.25" />
        </>
      ) : (
        <path d="M1.5 3.5h5l1.2 1.2H14v8.8H1.5V3.5z" fill={fill} />
      )}
    </svg>
  );
}

/** 按文件名返回文件类型 SVG 图标 */
export function FileTypeIcon({ name, className }: { name: string } & IconProps) {
  const ext = name.includes(".") ? (name.split(".").pop()?.toLowerCase() ?? "") : "";
  const base = name.toLowerCase();

  if (base === "package.json" || base === "package-lock.json") {
    return <BadgeIcon label="N" bg="#cb3837" fg="#fff" className={className} />;
  }
  if (base === "cargo.toml" || base === "cargo.lock") {
    return <BadgeIcon label="R" bg="#dea584" className={className} />;
  }
  if (base === "readme.md" || base.startsWith("readme.")) {
    return <BadgeIcon label="MD" bg="#519aba" fg="#fff" className={className} />;
  }

  switch (ext) {
    case "js":
    case "mjs":
    case "cjs":
      return <BadgeIcon label="JS" bg="#f7df1e" className={className} />;
    case "jsx":
      return <BadgeIcon label="JX" bg="#61dafb" className={className} />;
    case "ts":
      return <BadgeIcon label="TS" bg="#3178c6" fg="#fff" className={className} />;
    case "tsx":
      return <BadgeIcon label="TX" bg="#3178c6" fg="#fff" className={className} />;
    case "rs":
      return <BadgeIcon label="RS" bg="#dea584" className={className} />;
    case "json":
      return <BadgeIcon label="{}" bg="#cbcb41" className={className} />;
    case "toml":
      return <BadgeIcon label="TOML" bg="#848484" fg="#fff" className={className} />;
    case "yaml":
    case "yml":
      return <BadgeIcon label="YML" bg="#848484" fg="#fff" className={className} />;
    case "md":
    case "mdx":
      return <BadgeIcon label="MD" bg="#519aba" fg="#fff" className={className} />;
    case "css":
    case "scss":
    case "sass":
    case "less":
      return <BadgeIcon label="#" bg="#563d7c" fg="#fff" className={className} />;
    case "html":
    case "htm":
      return <BadgeIcon label="<>" bg="#e34c26" fg="#fff" className={className} />;
    case "py":
      return <BadgeIcon label="PY" bg="#3776ab" fg="#fff" className={className} />;
    case "go":
      return <BadgeIcon label="GO" bg="#00add8" fg="#fff" className={className} />;
    case "java":
      return <BadgeIcon label="J" bg="#b07219" fg="#fff" className={className} />;
    case "sql":
      return <BadgeIcon label="SQL" bg="#e38c00" fg="#fff" className={className} />;
    case "sh":
    case "bash":
    case "ps1":
      return <BadgeIcon label="$" bg="#4caf50" fg="#fff" className={className} />;
    case "png":
    case "jpg":
    case "jpeg":
    case "gif":
    case "webp":
    case "ico":
    case "svg":
      return <DocIcon color="#a074c4" className={className} />;
    case "lock":
      return <BadgeIcon label="LK" bg="#6e6e6e" fg="#fff" className={className} />;
    default:
      return <DocIcon color="#848484" className={className} />;
  }
}
