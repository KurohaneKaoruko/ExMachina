/** 列表骨架屏（visual-polish）：与列表条目同构的灰阶占位，shimmer 纯 CSS。
 *  variant = row（会话/清单行）| card（卡片网格） */
import React from "react";

export function SkeletonList({ rows = 6, variant = "row" }: { rows?: number; variant?: "row" | "card" }): React.ReactElement {
  if (variant === "card") {
    return (
      <div className="skill-grid" aria-hidden="true">
        {Array.from({ length: rows }, (_, i) => (
          <div key={i} className="skeleton-card">
            <div className="skeleton-line skeleton-line-title" />
            <div className="skeleton-line" style={{ width: "60%" }} />
            <div className="skeleton-line" style={{ width: "85%" }} />
            <div className="skeleton-line" style={{ width: "45%" }} />
          </div>
        ))}
      </div>
    );
  }
  return (
    <div className="skeleton-list" aria-hidden="true">
      {Array.from({ length: rows }, (_, i) => (
        <div key={i} className="skeleton-row">
          <div className="skeleton-line skeleton-line-title" />
          <div className="skeleton-line" style={{ width: "55%" }} />
        </div>
      ))}
    </div>
  );
}
