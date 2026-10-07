/** Markdown 渲染器（GFM），用于消息与回流面板。
 *  代码块带一键复制（chat-ergonomics）；复制走 Clipboard API + execCommand 降级。 */
import React, { useState } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";

/** 复制文本：优先 Clipboard API，非安全上下文降级 execCommand */
export async function copyText(text: string): Promise<boolean> {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    try {
      const ta = document.createElement("textarea");
      ta.value = text;
      ta.style.position = "fixed";
      ta.style.opacity = "0";
      document.body.appendChild(ta);
      ta.select();
      const ok = document.execCommand("copy");
      document.body.removeChild(ta);
      return ok;
    } catch {
      return false;
    }
  }
}

/** 代码块容器：右上角复制按钮，已复制短暂反馈 */
function CodeBlock(props: React.HTMLAttributes<HTMLPreElement>): React.ReactElement {
  const [copied, setCopied] = useState(false);
  const codeText = extractText(props.children);
  return (
    <div className="code-block-wrap">
      <button
        className="code-copy-btn"
        onClick={async () => {
          if (await copyText(codeText)) {
            setCopied(true);
            setTimeout(() => setCopied(false), 1200);
          }
        }}
        aria-label="copy"
      >
        {copied ? "已复制" : "复制"}
      </button>
      <pre {...props}>{props.children}</pre>
    </div>
  );
}

/** 递归抽取子节点的纯文本（代码块内容在 children 树里） */
function extractText(node: React.ReactNode): string {
  if (node == null || typeof node === "boolean") return "";
  if (typeof node === "string" || typeof node === "number") return String(node);
  if (Array.isArray(node)) return node.map(extractText).join("");
  if (React.isValidElement(node)) {
    return extractText((node.props as { children?: React.ReactNode }).children);
  }
  return "";
}

export function Markdown({ text }: { text: string }): React.ReactElement {
  return (
    <div className="md">
      <ReactMarkdown
        remarkPlugins={[remarkGfm]}
        components={{
          a: ({ node, ref, ...props }) => <a {...(props as object)} target="_blank" rel="noopener noreferrer" />,
          pre: (props) => <CodeBlock {...(props as React.HTMLAttributes<HTMLPreElement>)} />,
        }}
      >
        {text}
      </ReactMarkdown>
    </div>
  );
}
