/** 登录门：访问密钥鉴权 —— 四弧纹样徽记（纯白线条）+ 纯白等宽标题 + 硬朗扁平面板
 *
 *  扁平化科技感口径：纯黑底、纯白主元素、1px 边框直角面板，无渐变无辉光。
 *  动效保留（transform/opacity）：徽记缓旋、字节雨平移、入场浮升；prefers-reduced-motion 全局降级。
 */
import React, { useEffect, useMemo, useRef, useState } from "react";
import { Button, Input, type InputRef } from "antd";
import { KeyOutlined } from "@ant-design/icons";
import { api } from "../api";
import { Emblem } from "../components/Emblem";
import { useT } from "../i18n/core";

/** 数据流列：十六进制字节雨（纯白低透明，慢速，纯 transform 位移动效） */
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
          <Emblem size={148} animated />
        </div>
        <h1 className="login-title">EX-MACHINA</h1>
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
        <div className="login-foot mono">{t("login.foot")}</div>
      </div>
    </div>
  );
}
