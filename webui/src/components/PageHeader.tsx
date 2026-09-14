/** 统一页头：标题 + 一句话说明 + 右侧操作区（各面板共用，保证信息层级一致） */
import React from "react";

interface PageHeaderProps {
  title: string;
  /** 英文对照，显示在标题右侧 */
  en?: string;
  desc?: React.ReactNode;
  actions?: React.ReactNode;
}

export function PageHeader({ title, en, desc, actions }: PageHeaderProps): React.ReactElement {
  return (
    <div className="page-head">
      <div className="page-head-row">
        <div className="page-head-main">
          <div className="page-title-row">
            <h2 className="page-title">{title}</h2>
            {en ? <span className="page-en">{en}</span> : null}
          </div>
        </div>
        {actions ? <div className="page-actions">{actions}</div> : null}
      </div>
      {desc ? <p className="page-desc">{desc}</p> : null}
    </div>
  );
}
