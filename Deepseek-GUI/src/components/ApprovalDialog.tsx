// 审批弹窗组件：收到 approval.required 时展示，用户批准/拒绝后回传后端

import { useState } from "react";
import type { PendingApproval } from "../state/useConversation";

interface ApprovalDialogProps {
  approval: PendingApproval;
  onDecide: (approvalId: string, decision: "approve" | "reject", remember: boolean) => void;
}

export function ApprovalDialog({ approval, onDecide }: ApprovalDialogProps) {
  // 是否记住本次选择（对应后端 remember 字段；默认勾选以减少重复确认）
  const [remember, setRemember] = useState(true);

  return (
    <div className="modal-mask">
      <div className="modal">
        <h3 className="modal-title">{approval.title}</h3>
        {approval.detail && <pre className="modal-detail">{approval.detail}</pre>}
        <label className="modal-remember">
          <input
            type="checkbox"
            checked={remember}
            onChange={(e) => setRemember(e.target.checked)}
          />
          记住此选择
        </label>
        <div className="modal-actions">
          <button
            className="btn btn-danger"
            onClick={() => onDecide(approval.approvalId, "reject", remember)}
          >
            拒绝
          </button>
          <button
            className="btn btn-primary"
            onClick={() => onDecide(approval.approvalId, "approve", remember)}
          >
            批准
          </button>
        </div>
      </div>
    </div>
  );
}
