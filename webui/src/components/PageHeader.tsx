/** 统一页头：编号 + 标题 + 一句话说明 + 右侧操作区（各面板共用，保证信息层级一致）
 *  编号为该视图在侧栏清单中的序号，纯序号不造词。 */
import React from "react";

interface PageHeaderProps {
  /** 侧栏清单序号，如 "04"；缺省则不显示编号块 */
  no?: string;
  title: string;
  /** 英文对照，显示在标题右侧 */
  en?: string;
  desc?: React.ReactNode;
  actions?: React.ReactNode;
}

export function PageHeader({ no, title, en, desc, actions }: PageHeaderProps): React.ReactElement {
  return (
    <div className="page-head">
      <div className="page-head-row">
        <div className="page-head-main">
          <div className="page-title-row">
            {no ? <span className="page-no">{no}</span> : null}
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
