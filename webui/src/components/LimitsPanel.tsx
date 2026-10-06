/** 用量治理面板（capability-completion 组 4.4）：限流计数快照 + 配置口径展示。
 *  数据源：/api/limits/stats（配置经设置页 limits 组热生效） */
import React, { useCallback, useEffect, useState } from "react";
import { Button, Card, Tag } from "antd";
import { ReloadOutlined, ThunderboltOutlined } from "@ant-design/icons";
import { limitsStats } from "../api";
import { useT } from "../i18n/core";

interface LimitsStats {
  enabled: boolean;
  windowSecs: number;
  maxRequests: number;
  quotaPeriod: string;
  quotaTokens: number;
  quotaRequests: number;
  counters: { key: string; count: number }[];
}

export function LimitsPanel(): React.ReactElement {
  const t = useT();
  const [stats, setStats] = useState<LimitsStats | null>(null);

  const load = useCallback(async () => {
    try {
      setStats(await limitsStats());
    } catch {
      /* 静默 */
    }
  }, []);
  useEffect(() => {
    void load();
  }, [load]);

  return (
    <Card
      size="small"
      className="limits-panel"
      title={<><ThunderboltOutlined /> {t("limits.title")}</>}
      extra={<Button size="small" type="text" icon={<ReloadOutlined />} onClick={() => void load()} />}
    >
      {!stats ? (
        <span className="dim">{t("limits.unavailable")}</span>
      ) : (
        <div className="limits-body">
          <div className="limits-row">
            <Tag color={stats.enabled ? "processing" : "default"}>{stats.enabled ? t("limits.on") : t("limits.off")}</Tag>
            <span className="dim mono">
              window {stats.windowSecs}s · max {stats.maxRequests || "∞"} · {stats.quotaPeriod} tokens {stats.quotaTokens || "∞"} / req {stats.quotaRequests || "∞"}
            </span>
          </div>
          {stats.counters.length === 0 ? (
            <span className="dim">{t("limits.noCounters")}</span>
          ) : (
            <div className="limits-counters mono">
              {stats.counters.map((c) => (
                <div key={c.key} className="limits-counter">
                  <span className="limits-key">{c.key}</span>
                  <span className="limits-count">{c.count}</span>
                </div>
              ))}
            </div>
          )}
        </div>
      )}
    </Card>
  );
}
