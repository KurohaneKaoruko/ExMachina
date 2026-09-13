/** 登录门：访问密钥鉴权 —— 大标题 + 密钥框 + 等高线智械体 + 数据流装饰 */
import React, { useEffect, useMemo, useRef, useState } from "react";
import { Button, Input, type InputRef } from "antd";
import { KeyOutlined } from "@ant-design/icons";
import { api } from "../api";

/** 等高线智械体：多层轮廓缩放 + 上移偏移 = 伪 3D 浮雕 */
function ContourMachina(): React.ReactElement {
  const bust =
    "M120,18 C86,18 64,42 62,76 C61,96 66,112 74,124 C80,133 82,140 80,150 L76,166 C60,174 40,182 30,196 C20,210 16,228 16,244 L224,244 C224,228 220,210 210,196 C200,182 180,174 164,166 L160,150 C158,140 160,133 166,124 C174,112 179,96 178,76 C176,42 154,18 120,18 Z";
  const levels = [1.0, 0.86, 0.72, 0.58, 0.45];
  return (
    <svg className="contour-machina" viewBox="0 0 240 260" width={230} height={250}>
      <defs>
        <clipPath id="bust-clip">
          <path d={bust} />
        </clipPath>
      </defs>
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
      {/* 护目镜与传感线 */}
      <g clipPath="url(#bust-clip)">
        <rect x={76} y={76} width={88} height={13} rx={2} fill="none" stroke="var(--accent)" strokeWidth={1.4} opacity={0.85} />
        <line x1={120} y1={96} x2={120} y2={148} stroke="var(--accent)" strokeWidth={0.8} opacity={0.4} />
        <line x1={92} y1={104} x2={92} y2={140} stroke="var(--accent)" strokeWidth={0.8} opacity={0.3} />
        <line x1={148} y1={104} x2={148} y2={140} stroke="var(--accent)" strokeWidth={0.8} opacity={0.3} />
        {/* 扫描线 */}
        <line className="contour-scan" x1={10} y1={0} x2={230} y2={0} stroke="var(--accent)" strokeWidth={1.2} opacity={0.9} />
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
        setError(r.error ?? "密钥错误");
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
        <div className="login-sub">
          智械体集群 // 智械集群 <span className="mono">[SECURE ACCESS]</span>
        </div>
        <div className="login-box hud">
          <div className="login-label">
            <KeyOutlined /> 访问密钥 <span className="label-en">[ACCESS KEY]</span>
          </div>
          <Input.Password
            ref={inputRef}
            value={key}
            onChange={(e) => setKey(e.target.value)}
            placeholder="> 输入访问密钥"
            onPressEnter={() => void unlock()}
            status={error ? "error" : undefined}
          />
          <Button type="primary" block loading={loading} onClick={() => void unlock()} className="login-btn">
            解锁进入 <span className="label-en">[UNLOCK]</span>
          </Button>
          {error && <div className="login-error">{error}</div>}
        </div>
        <div className="login-foot mono">DEUS EX MACHINA · 全连结指挥就绪</div>
      </div>
    </div>
  );
}
