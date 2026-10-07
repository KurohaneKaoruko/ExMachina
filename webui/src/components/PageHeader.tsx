/** 统一页头：标题 + 一句话说明 + 右侧操作区（各面板共用，保证信息层级一致）
 *
 *  字符画化改造：底部那条 1px 实线换成 「═」通栏字符带。
 *  CSS 实线是连续的图形，字符带是离散的格子 —— 后者读起来像"机器打印出来的一行"，
 *  这是本项目「程序编码」风格的核心区别点。
 */
import React from "react";
import { AsciiBar } from "./Ascii";

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
          {desc ? <p className="page-desc">{desc}</p> : null}
        </div>
        {actions ? <div className="page-actions">{actions}</div> : null}
      </div>
      <AsciiBar className="page-head-ascii" />
    </div>
  );
}
