/** 登录门：访问密钥鉴权 —— 大标题 + 密钥框 + 等高线智械体 + 数据流装饰 */
import React, { useEffect, useMemo, useRef, useState } from "react";
import { Button, Input, type InputRef } from "antd";
import { KeyOutlined } from "@ant-design/icons";
import { api } from "../api";
import { useT } from "../i18n/core";

/** 等高线智械体：多层轮廓缩放 + 上移偏移 = 伪 3D 浮雕；外加坐标轴与刻度环 */
function ContourMachina(): React.ReactElement {
  const bust =
    "M120,18 C86,18 64,42 62,76 C61,96 66,112 74,124 C80,133 82,140 80,150 L76,166 C60,174 40,182 30,196 C20,210 16,228 16,244 L224,244 C224,228 220,210 210,196 C200,182 180,174 164,166 L160,150 C158,140 160,133 166,124 C174,112 179,96 178,76 C176,42 154,18 120,18 Z";
  const levels = [1.0, 0.86, 0.72, 0.58, 0.45];
  // 刻度环：每 10° 一条刻线
  const ticks = Array.from({ length: 72 }, (_, i) => i * 5);
  return (
    <svg className="contour-machina" viewBox="0 0 260 280" width={248} height={268}>
      <defs>
        <clipPath id="bust-clip">
          <path d={bust} transform="translate(10,0)" />
        </clipPath>
      </defs>

      {/* 坐标轴：十字准星 + 端部刻度 */}
      <g opacity="0.35" stroke="var(--accent)" strokeWidth="0.8">
        <line x1={130} y1={4} x2={130} y2={272} strokeDasharray="2 6" />
        <line x1={4} y1={140} x2={256} y2={140} strokeDasharray="2 6" />
      </g>

      {/* 刻度环：围绕智械体的量度刻线 */}
      <g transform="translate(130,140)" opacity="0.5">
        {ticks.map((deg, i) => {
          const major = deg % 30 === 0;
          const rad = (deg * Math.PI) / 180;
          const r1 = 118;
          const r2 = major ? 128 : 123;
          return (
            <line
              key={i}
              x1={Math.cos(rad) * r1}
              y1={Math.sin(rad) * r1}
              x2={Math.cos(rad) * r2}
              y2={Math.sin(rad) * r2}
              stroke="var(--accent)"
              strokeWidth={major ? 1 : 0.6}
              opacity={major ? 0.65 : 0.3}
            />
          );
        })}
        <circle r={118} fill="none" stroke="var(--accent)" strokeWidth="0.6" opacity="0.25" />
        <circle r={128} fill="none" stroke="var(--accent)" strokeWidth="0.6" opacity="0.15" />
      </g>

      <g transform="translate(10,0)">
        {levels.map((s, i) => (
          <path
            key={i}
            d={bust}
            fill="none"
            stroke="var(--accent)"
            strokeWidth={i === 0 ? 1.6 : 1}
            opacity={0.14 + i * 0.09}
            transform={`translate(${120 * (1 - s)}, ${140 * (1 - s) - i * 5}) scale(${s})`}
          />
        ))}
      </g>
      {/* 护目镜与传感线 */}
      <g clipPath="url(#bust-clip)">
        <rect x={86} y={76} width={88} height={13} rx={0} fill="none" stroke="var(--accent)" strokeWidth={1.4} opacity={0.85} />
        <line x1={130} y1={96} x2={130} y2={148} stroke="var(--accent)" strokeWidth={0.8} opacity={0.4} />
        <line x1={102} y1={104} x2={102} y2={140} stroke="var(--accent)" strokeWidth={0.8} opacity={0.3} />
        <line x1={158} y1={104} x2={158} y2={140} stroke="var(--accent)" strokeWidth={0.8} opacity={0.3} />
        {/* 扫描线 */}
        <line className="contour-scan" x1={0} y1={0} x2={260} y2={0} stroke="var(--accent)" strokeWidth={1.2} opacity={0.9} />
      </g>
    </svg>
  );
}

/** 数据流列：十六进制字节雨（低透明度，慢速） */
function DataStream({ side }: { side: "left" | "right" }): React.ReactElement {
  const lines = useMemo(() => {
    const rows: string[] = [];
    for (let i = 0; i < 28; i++) {
      let s = "";
      for (let j = 0; j < 8; j++) {
        s += Math.floor(Math.random() * 256)
          .toString(16)
          .padStart(2, "0")
          .toUpperCase() + " ";
      }
      rows.push(s);
    }
    return rows;
  }, []);
  return (
    <div className={`data-stream data-stream-${side}`}>
      <div className="data-stream-inner">
        {[0, 1].map((dup) => (
          <div key={dup}>
            {lines.map((l, i) => (
              <div key={i} className="data-line">
                {l}
              </div>
            ))}
          </div>
        ))}
      </div>
    </div>
  );
}

export function LoginView({ onUnlock }: { onUnlock: () => void }): React.ReactElement {
  const [key, setKey] = useState("");
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");
  const inputRef = useRef<InputRef>(null);
  const t = useT();

  useEffect(() => {
    inputRef.current?.focus();
  }, []);

  const unlock = async () => {
    if (!key.trim() || loading) return;
    setLoading(true);
    setError("");
    try {
      const r = await api.verifyAuth(key.trim());
      if (r.ok) {
        localStorage.setItem("exm.key", key.trim());
        onUnlock();
      } else {
        setError(r.error ?? t("login.badKey"));
      }
    } catch (e) {
      setError(String(e).slice(0, 120));
    } finally {
      setLoading(false);
    }
  };

  return (
    <div className="login-wrap">
      <DataStream side="left" />
      <DataStream side="right" />
      <div className="login-core">
        <div className="login-figure">
          <ContourMachina />
        </div>
        <h1 className="login-title">
          <span className="title-accent">EX</span>
          <span className="title-dash">-</span>
          <span className="title-main">MACHINA</span>
        </h1>
        <div className="title-rule" />
        <div className="login-sub">
          {t("login.sub")} <span className="mono">[SECURE ACCESS]</span>
        </div>
        <div className="login-box hud">
          <div className="login-label">
            <KeyOutlined /> {t("login.keyLabel")} <span className="label-en">[ACCESS KEY]</span>
          </div>
          <Input.Password
            ref={inputRef}
            value={key}
            onChange={(e) => setKey(e.target.value)}
            placeholder={`> ${t("login.keyPlaceholder")}`}
            onPressEnter={() => void unlock()}
            status={error ? "error" : undefined}
          />
          <Button type="primary" block loading={loading} onClick={() => void unlock()} className="login-btn">
            {t("login.unlock")} <span className="label-en">[UNLOCK]</span>
          </Button>
          {error && <div className="login-error">{error}</div>}
        </div>
        <div className="login-foot mono">{t("login.foot")}</div>      </div>
    </div>
  );
}
