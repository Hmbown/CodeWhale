// 文件树组件：惰性展开目录（每次只读一层），点击文件回调打开
// 依赖壳层命令 list_dir 读盘，仅在 Tauri 桌面环境可用
// 支持右键菜单新建/重命名/删除

import { useCallback, useEffect, useState } from "react";
import { deletePath, createDir, createFile, formatInvokeError, listDir, renamePath, type DirEntryInfo } from "../api/tauri";
import { FileTypeIcon, FolderIcon, TreeChevron } from "../utils/fileIcons";

interface FileTreeProps {
  /** 项目根目录绝对路径 */
  rootPath: string;
  /** 当前打开的文件路径（用于高亮选中项） */
  activePath: string | null;
  /** 打开文件回调 */
  onOpenFile: (path: string) => void;
  /** 刷新令牌：变化时所有已展开目录重新读盘 */
  reloadToken: number;
  /** 文件树发生变更（如删除）后回调，用于触发刷新 */
  onChanged: () => void;
  /** 某个路径被删除后回调（用于关闭正在查看的已删除文件） */
  onDeleted: (path: string) => void;
  /** 某个路径被重命名后回调（用于同步编辑器标签） */
  onRenamed?: (from: string, to: string) => void;
}

/** 右键菜单所针对的文件/文件夹条目 */
interface ContextItem {
  path: string;
  name: string;
  isDir: boolean;
}

/** 右键菜单状态 */
interface ContextMenuState {
  x: number;
  y: number;
  /** 新建文件/文件夹的目标目录 */
  targetDir: string;
  /** 右键点击的具体条目（用于重命名/删除） */
  item?: ContextItem;
}

/** 打开右键菜单的回调签名 */
type OpenContextMenu = (e: React.MouseEvent, targetDir: string, item?: ContextItem) => void;

/** 文件树根：直接渲染根目录的子节点 */
export function FileTree({
  rootPath,
  activePath,
  onOpenFile,
  reloadToken,
  onChanged,
  onDeleted,
  onRenamed,
}: FileTreeProps) {
  const [menu, setMenu] = useState<ContextMenuState | null>(null);

  const closeMenu = useCallback(() => setMenu(null), []);

  /** 在指定目录下弹出右键菜单 */
  const openContextMenu = useCallback<OpenContextMenu>((e, targetDir, item) => {
    e.preventDefault();
    e.stopPropagation();
    setMenu({ x: e.clientX, y: e.clientY, targetDir, item });
  }, []);

  /** 在 targetDir 下新建文件 */
  const handleNewFile = useCallback(
    async (targetDir: string) => {
      closeMenu();
      const name = window.prompt("新建文件名", "untitled.txt");
      if (!name?.trim()) return;
      const path = joinPath(targetDir, name.trim());
      try {
        await createFile(path, "");
        onChanged();
        onOpenFile(path);
      } catch (err) {
        alert(`创建失败：${formatInvokeError(err)}`);
      }
    },
    [closeMenu, onChanged, onOpenFile],
  );

  /** 在 targetDir 下新建文件夹 */
  const handleNewFolder = useCallback(
    async (targetDir: string) => {
      closeMenu();
      const name = window.prompt("新建文件夹名", "新建文件夹");
      if (!name?.trim()) return;
      const path = joinPath(targetDir, name.trim());
      try {
        await createDir(path);
        onChanged();
      } catch (err) {
        alert(`创建失败：${formatInvokeError(err)}`);
      }
    },
    [closeMenu, onChanged],
  );

  /** 点击页面其它区域时关闭右键菜单 */
  useEffect(() => {
    if (!menu) return;
    const close = () => closeMenu();
    window.addEventListener("click", close);
    window.addEventListener("contextmenu", close);
    window.addEventListener("scroll", close, true);
    return () => {
      window.removeEventListener("click", close);
      window.removeEventListener("contextmenu", close);
      window.removeEventListener("scroll", close, true);
    };
  }, [menu, closeMenu]);

  return (
    <div
      className="file-tree"
      onContextMenu={(e) => openContextMenu(e, rootPath)}
    >
      <DirChildren
        dir={rootPath}
        depth={0}
        activePath={activePath}
        onOpenFile={onOpenFile}
        reloadToken={reloadToken}
        onChanged={onChanged}
        onDeleted={onDeleted}
        onRenamed={onRenamed}
        openContextMenu={openContextMenu}
      />
      {menu && (
        <TreeContextMenu
          state={menu}
          onNewFile={() => void handleNewFile(menu.targetDir)}
          onNewFolder={() => void handleNewFolder(menu.targetDir)}
          onRename={
            menu.item
              ? () => {
                  closeMenu();
                  void handleRename(
                    menu.item!.path,
                    menu.item!.name,
                    menu.item!.isDir,
                    onChanged,
                    onRenamed,
                  );
                }
              : undefined
          }
          onDelete={
            menu.item
              ? () => {
                  closeMenu();
                  void handleDelete(
                    menu.item!.path,
                    menu.item!.name,
                    menu.item!.isDir,
                    onChanged,
                    onDeleted,
                  );
                }
              : undefined
          }
        />
      )}
    </div>
  );
}

/** 右键上下文菜单 */
function TreeContextMenu({
  state,
  onNewFile,
  onNewFolder,
  onRename,
  onDelete,
}: {
  state: ContextMenuState;
  onNewFile: () => void;
  onNewFolder: () => void;
  onRename?: () => void;
  onDelete?: () => void;
}) {
  return (
    <div
      className="tree-ctx-menu"
      style={{ left: state.x, top: state.y }}
      onClick={(e) => e.stopPropagation()}
      onContextMenu={(e) => e.preventDefault()}
    >
      <button type="button" onClick={onNewFile}>
        新建文件
      </button>
      <button type="button" onClick={onNewFolder}>
        新建文件夹
      </button>
      {onRename && (
        <>
          <div className="tree-ctx-sep" />
          <button type="button" onClick={onRename}>
            重命名
          </button>
        </>
      )}
      {onDelete && (
        <button type="button" className="tree-ctx-danger" onClick={onDelete}>
          删除
        </button>
      )}
    </div>
  );
}

/** 公共子节点属性（在树中逐层透传） */
interface NodeCommon {
  activePath: string | null;
  onOpenFile: (path: string) => void;
  reloadToken: number;
  onChanged: () => void;
  onDeleted: (path: string) => void;
  onRenamed?: (from: string, to: string) => void;
  openContextMenu: OpenContextMenu;
}

/**
 * 删除某个文件或文件夹：弹确认框，调用壳层 delete_path，成功后触发刷新。
 */
async function handleDelete(
  path: string,
  name: string,
  isDir: boolean,
  onChanged: () => void,
  onDeleted: (path: string) => void,
) {
  const kind = isDir ? "文件夹（含其全部内容）" : "文件";
  if (!window.confirm(`确定删除${kind}「${name}」吗？\n此操作不可恢复。`)) return;
  try {
    await deletePath(path);
    onDeleted(path);
    onChanged();
  } catch (err) {
    alert(`删除失败：${formatInvokeError(err)}`);
  }
}

/** 重命名文件或文件夹 */
async function handleRename(
  path: string,
  name: string,
  isDir: boolean,
  onChanged: () => void,
  onRenamed?: (from: string, to: string) => void,
) {
  const next = window.prompt(`重命名${isDir ? "文件夹" : "文件"}`, name);
  if (next == null) return;
  const trimmed = next.trim();
  if (!trimmed || trimmed === name) return;
  const parent = parentDir(path);
  const to = joinPath(parent, trimmed);
  try {
    await renamePath(path, to);
    onRenamed?.(path, to);
    onChanged();
  } catch (err) {
    alert(`重命名失败：${formatInvokeError(err)}`);
  }
}

/** 取路径的父目录 */
function parentDir(path: string): string {
  return path.replace(/[\\/][^\\/]+$/, "");
}

/** 拼接路径（兼容 Windows 反斜杠） */
function joinPath(base: string, rel: string): string {
  const sep = base.includes("\\") ? "\\" : "/";
  return `${base.replace(/[\\/]+$/, "")}${sep}${rel.replace(/^[./\\]+/, "")}`;
}

/** 渲染某个目录的子节点列表（惰性加载） */
function DirChildren({
  dir,
  depth,
  activePath,
  onOpenFile,
  reloadToken,
  onChanged,
  onDeleted,
  onRenamed,
  openContextMenu,
}: { dir: string; depth: number } & NodeCommon) {
  const [entries, setEntries] = useState<DirEntryInfo[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let alive = true;
    listDir(dir)
      .then((e) => alive && setEntries(e))
      .catch((err) => alive && setError(formatInvokeError(err)));
    return () => {
      alive = false;
    };
    // reloadToken 变化时重新读盘，反映 agent 新建/删除的文件
  }, [dir, reloadToken]);

  /** 空白/加载/错误区域右键：在该目录下新建 */
  const onBlankContext = (e: React.MouseEvent) => openContextMenu(e, dir);

  if (error) {
    return (
      <div className="tree-error" style={indent(depth)} onContextMenu={onBlankContext}>
        {error}
      </div>
    );
  }
  if (!entries) {
    return (
      <div className="tree-loading" style={indent(depth)} onContextMenu={onBlankContext}>
        加载中…
      </div>
    );
  }
  if (entries.length === 0) {
    return (
      <div className="tree-empty" style={indent(depth)} onContextMenu={onBlankContext}>
        （空）
      </div>
    );
  }

  return (
    <>
      {entries.map((e) =>
        e.is_dir ? (
          <DirNode
            key={e.path}
            entry={e}
            depth={depth}
            activePath={activePath}
            onOpenFile={onOpenFile}
            reloadToken={reloadToken}
            onChanged={onChanged}
            onDeleted={onDeleted}
            onRenamed={onRenamed}
            openContextMenu={openContextMenu}
          />
        ) : (
          <div
            key={e.path}
            className={`tree-row tree-file${e.path === activePath ? " active" : ""}`}
            style={rowDepth(depth)}
            onClick={() => onOpenFile(e.path)}
            onContextMenu={(ev) =>
              openContextMenu(ev, parentDir(e.path), {
                path: e.path,
                name: e.name,
                isDir: false,
              })
            }
            title={e.path}
          >
            <span className="tree-chevron-slot" aria-hidden />
            <FileTypeIcon name={e.name} className="tree-file-icon" />
            <span className="tree-name">{e.name}</span>
            <button
              className="tree-act"
              title="重命名"
              onClick={(ev) => {
                ev.stopPropagation();
                void handleRename(e.path, e.name, false, onChanged, onRenamed);
              }}
            >
              ✎
            </button>
            <button
              className="tree-del"
              title="删除"
              onClick={(ev) => {
                ev.stopPropagation();
                void handleDelete(e.path, e.name, false, onChanged, onDeleted);
              }}
            >
              🗑
            </button>
          </div>
        ),
      )}
    </>
  );
}

/** 可展开/折叠的目录节点 */
function DirNode({
  entry,
  depth,
  activePath,
  onOpenFile,
  reloadToken,
  onChanged,
  onDeleted,
  onRenamed,
  openContextMenu,
}: { entry: DirEntryInfo; depth: number } & NodeCommon) {
  const [open, setOpen] = useState(false);
  const toggle = useCallback(() => setOpen((v) => !v), []);
  return (
    <div className="tree-dir-wrap">
      <div
        className="tree-row tree-dir"
        style={rowDepth(depth)}
        onClick={toggle}
        onContextMenu={(ev) =>
          openContextMenu(ev, entry.path, {
            path: entry.path,
            name: entry.name,
            isDir: true,
          })
        }
        title={entry.path}
      >
        <span className="tree-chevron-slot">
          <TreeChevron open={open} className="tree-chevron" />
        </span>
        <FolderIcon name={entry.name} open={open} className="tree-file-icon" />
        <span className="tree-name">{entry.name}</span>
        <button
          className="tree-act"
          title="重命名"
          onClick={(ev) => {
            ev.stopPropagation();
            void handleRename(entry.path, entry.name, true, onChanged, onRenamed);
          }}
        >
          ✎
        </button>
        <button
          className="tree-del"
          title="删除文件夹"
          onClick={(ev) => {
            ev.stopPropagation();
            void handleDelete(entry.path, entry.name, true, onChanged, onDeleted);
          }}
        >
          🗑
        </button>
      </div>
      {open && (
        <div className="tree-children" style={rowDepth(depth)}>
          <DirChildren
            dir={entry.path}
            depth={depth + 1}
          activePath={activePath}
          onOpenFile={onOpenFile}
          reloadToken={reloadToken}
          onChanged={onChanged}
          onDeleted={onDeleted}
          onRenamed={onRenamed}
          openContextMenu={openContextMenu}
        />
        </div>
      )}
    </div>
  );
}

/** 树行缩进（每层 12px，仿 Cursor/VS Code） */
function rowDepth(depth: number): React.CSSProperties {
  return { ["--tree-depth" as string]: depth };
}

/** @deprecated 保留给 loading/error 占位 */
function indent(depth: number): React.CSSProperties {
  return { paddingLeft: `${8 + depth * 12}px` };
}
