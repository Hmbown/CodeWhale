"use client";

import { useState } from "react";
import { draftStorageKey, type AgentDraft } from "@/lib/community-agent";

interface Props {
  drafts: AgentDraft[];
  posted: AgentDraft[];
  isZh: boolean;
  typeLabels: Record<string, { en: string; zh: string }>;
}

export function AdminClient({ drafts, posted, isZh, typeLabels }: Props) {
  const [items, setItems] = useState(drafts);
  const [postedItems, setPostedItems] = useState(posted);
  const [editing, setEditing] = useState<string | null>(null);
  const [editBody, setEditBody] = useState("");
  const [loading, setLoading] = useState<string | null>(null);

  const handleAction = async (draftKey: string, action: "post" | "discard", editedBody?: string) => {
    setLoading(draftKey);
    try {
      const res = await fetch("/api/admin/post", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ action, draftKey, editedBody, lang: isZh ? "zh" : "en" }),
      });
      const data = await res.json();
      if (data.ok) {
        if (action === "discard") {
          setItems((prev) => prev.filter((d) => draftStorageKey(d) !== draftKey));
        } else if (action === "post") {
          const posted = items.find((d) => draftStorageKey(d) === draftKey);
          if (posted) {
            setItems((prev) => prev.filter((d) => draftStorageKey(d) !== draftKey));
            setPostedItems((prev) => [{ ...posted, posted: true }, ...prev]);
          }
        }
        setEditing(null);
      } else {
        alert(`Error: ${data.error}`);
      }
    } catch (e) {
      alert(`Network error: ${e}`);
    } finally {
      setLoading(null);
    }
  };

  const startEdit = (draft: AgentDraft) => {
    const key = draftStorageKey(draft);
    setEditing(key);
    setEditBody(isZh ? draft.bodyZh : draft.bodyEn);
  };

  return (
    <>
      {/* Pending drafts */}
      {items.length > 0 && (
        <div className="space-y-6">
          <h2 className="font-display text-xl mt-8 mb-4">
            {isZh ? "待审阅" : "Pending"} <span className="font-mono text-sm text-ink-mute ml-2">({items.length})</span>
          </h2>
          {items.map((draft) => {
            const key = draftStorageKey(draft);
            const label = typeLabels[draft.type] ?? { en: draft.type, zh: draft.type };
            return (
              <div key={key} className="hairline-t hairline-b hairline-l hairline-r bg-paper">
                <div className="bg-ink text-paper px-4 py-2 flex items-center justify-between">
                  <div className="flex items-center gap-3">
                    <span className="font-mono text-xs uppercase tracking-wider text-indigo">
                      {isZh ? label.zh : label.en}
                    </span>
                    {draft.targetNumber && (
                      <span className="font-mono text-xs text-paper-deep/70 tabular">#{draft.targetNumber}</span>
                    )}
                  </div>
                  <span className="font-mono text-xs text-paper-deep/50">
                    {new Date(draft.generatedAt).toISOString().slice(0, 16)}
                  </span>
                </div>

                <div className="p-4">
                  {editing === key ? (
                    <div className="space-y-3">
                      <textarea
                        value={editBody}
                        onChange={(e) => setEditBody(e.target.value)}
                        className="w-full h-48 p-3 bg-paper-deep hairline-t hairline-b hairline-l hairline-r font-mono text-sm resize-y"
                      />
                      <div className="flex gap-2">
                        <button
                          onClick={() => handleAction(key, "post", editBody)}
                          disabled={loading === key}
                          className="px-4 py-2 bg-indigo text-paper font-mono text-xs uppercase tracking-wider hover:bg-indigo-deep transition-colors disabled:opacity-50"
                        >
                          {isZh ? "确认发布" : "Post edited"}
                        </button>
                        <button
                          onClick={() => setEditing(null)}
                          className="px-4 py-2 hairline-t hairline-b hairline-l hairline-r font-mono text-xs uppercase tracking-wider hover:bg-paper-deep transition-colors"
                        >
                          {isZh ? "取消" : "Cancel"}
                        </button>
                      </div>
                    </div>
                  ) : (
                    <>
                      <div className="prose prose-sm max-w-none text-sm text-ink-soft leading-relaxed whitespace-pre-wrap mb-4">
                        {isZh ? draft.bodyZh : draft.bodyEn}
                      </div>
                      <div className="flex gap-2 flex-wrap">
                        <button
                          onClick={() => handleAction(key, "post")}
                          disabled={loading === key}
                          className="px-4 py-2 bg-ink text-paper font-mono text-xs uppercase tracking-wider hover:bg-indigo transition-colors disabled:opacity-50"
                        >
                          {isZh ? "发布评论" : "Post as comment"}
                        </button>
                        <button
                          onClick={() => startEdit(draft)}
                          className="px-4 py-2 hairline-t hairline-b hairline-l hairline-r font-mono text-xs uppercase tracking-wider hover:bg-paper-deep transition-colors"
                        >
                          {isZh ? "编辑后发布" : "Edit & post"}
                        </button>
                        <button
                          onClick={() => handleAction(key, "discard")}
                          disabled={loading === key}
                          className="px-4 py-2 font-mono text-xs uppercase tracking-wider text-ink-mute hover:text-indigo transition-colors disabled:opacity-50"
                        >
                          {isZh ? "丢弃" : "Discard"}
                        </button>
                      </div>
                    </>
                  )}
                </div>
              </div>
            );
          })}
        </div>
      )}

      {/* Posted drafts */}
      {postedItems.length > 0 && (
        <div className="mt-12">
          <h2 className="font-display text-xl mb-4">
            {isZh ? "已发布" : "Posted"} <span className="font-mono text-sm text-ink-mute ml-2">({postedItems.length})</span>
          </h2>
          {postedItems.map((draft) => {
            const key = draftStorageKey(draft);
            const label = typeLabels[draft.type] ?? { en: draft.type, zh: draft.type };
            return (
              <div key={key} className="hairline-t py-3 px-4 opacity-60">
                <div className="flex items-center gap-3 mb-1">
                  <span className="font-mono text-xs uppercase tracking-wider text-ink-mute">
                    {isZh ? label.zh : label.en}
                  </span>
                  {draft.targetNumber && (
                    <span className="font-mono text-xs text-ink-mute tabular">#{draft.targetNumber}</span>
                  )}
                  <span className="ml-auto pill pill-jade text-[0.6rem]">{isZh ? "已发布" : "posted"}</span>
                </div>
                <p className="text-xs text-ink-mute line-clamp-2">{(isZh ? draft.bodyZh : draft.bodyEn).slice(0, 120)}…</p>
              </div>
            );
          })}
        </div>
      )}
    </>
  );
}
