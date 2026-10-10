/** 底边遥测条 —— 品牌常驻系统读数（链路 / 会话 / 模型 / 版本）。
 *
 *  与桌面端底栏同一套 HUD 语汇：等宽大写键 + 语义色 LED + 右侧系统簇，
 *  让 Web 控制台与桌面客户端读同一组系统读数、看起来是同一件产品。
 *
 *  只消费 store 的只读状态；版本读数拉取一次，失败静默（保持 dev 占位）。
 */
import React, { useEffect, useState } from "react";
import { api } from "../api";
import { useExm } from "../store";
import { useT } from "../i18n/core";
import { Emblem } from "./Emblem";

export function TelemetryBar(): React.ReactElement {
  const wsConnected = useExm((s) => s.wsConnected);
  const sessions = useExm((s) => s.sessions);
  const config = useExm((s) => s.config);
  const t = useT();
  const [ver, setVer] = useState("");

  useEffect(() => {
    api.versionInfo()
      .then((v) => setVer(v.version ?? ""))
      .catch(() => {});
  }, []);

  const llmReady = Boolean(config?.llm?.apiKey);
  const model = config?.llm?.model?.trim() || (llmReady ? "—" : t("status.unlinked"));

  return (
    <div className="telemetry-bar" aria-hidden="true">
      <span className="tbar-brand">
        <Emblem size={12} animated={false} />
        EXM
      </span>
      <span className="tbar-seg">
        <span className={`tbar-led${wsConnected ? "" : " err"}`} />
        <span className="tbar-k">LINK</span>
        <span className={`tbar-v ${wsConnected ? "ok" : "err"}`}>{wsConnected ? "ONLINE" : "RECONN"}</span>
      </span>
      <span className="tbar-seg">
        <span className="tbar-k">SESS</span>
        <span className="tbar-v">{String(sessions.length).padStart(2, "0")}</span>
      </span>
      <span className="tbar-seg">
        <span className="tbar-k">MODEL</span>
        <span className={`tbar-v ${llmReady ? "" : "warn"}`}>{model}</span>
      </span>
      <span className="tbar-spacer" />
      <span className="tbar-seg sys">
        <span className="tbar-k">VER</span>
        <span className="tbar-v">{ver || "dev"}</span>
      </span>
    </div>
  );
}
