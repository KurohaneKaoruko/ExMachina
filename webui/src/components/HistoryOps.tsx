/** 会话历史管控操作条（组 9.5）：用户消息上的 撤销 / 编辑重发 / 分叉 入口 */
import React from "react";
import { Button, Popconfirm, Tooltip } from "antd";
import { EditOutlined, ForkOutlined, RollbackOutlined } from "@ant-design/icons";
import { useT } from "../i18n/core";

export interface TurnOpsProps {
  /** 该用户消息所在轮次（从 1 起） */
  turn: number;
  onEdit: (turn: number) => void;
  onUndo: (turn: number) => Promise<void>;
  onFork: (turn: number) => Promise<void>;
}

export function TurnOps({ turn, onEdit, onUndo, onFork }: TurnOpsProps): React.ReactElement {
  const t = useT();
  return (
    <span className="turn-ops" onClick={(e) => e.stopPropagation()}>
      <Tooltip title={t("chat.turn.edit")}>
        <Button
          size="small"
          type="text"
          className="turn-op"
          icon={<EditOutlined />}
          onClick={() => onEdit(turn)}
        />
      </Tooltip>
      <Popconfirm
        title={t("chat.turn.undoConfirm", { turn: String(turn - 1) })}
        onConfirm={() => void onUndo(turn)}
      >
        <Tooltip title={t("chat.turn.undo")}>
          <Button size="small" type="text" className="turn-op" icon={<RollbackOutlined />} />
        </Tooltip>
      </Popconfirm>
      <Tooltip title={t("chat.turn.fork")}>
        <Button
          size="small"
          type="text"
          className="turn-op"
          icon={<ForkOutlined />}
          onClick={() => void onFork(turn)}
        />
      </Tooltip>
    </span>
  );
}
