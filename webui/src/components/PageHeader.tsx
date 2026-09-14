/** 统一页头：标题 + 一句话说明 + 右侧操作区（各面板共用，保证信息层级一致） */
import React from "react";

interface PageHeaderProps {
  title: string;
  desc?: React.ReactNode;
  actions?: React.ReactNode;
}

export function PageHeader({ title, desc, actions }: PageHeaderProps): React.ReactElement {
  return (
    <div className="page-head">
      <div className="page-head-main">
        <h2 className="page-title">{title}</h2>
        {desc ? <p className="page-desc">{desc}</p> : null}
      </div>
      {actions ? <div className="page-actions">{actions}</div> : null}
    </div>
  );
}
