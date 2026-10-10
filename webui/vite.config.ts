import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

/** react 系（react/react-dom 生态 + 状态管理）：地基 chunk，其余 vendor 依赖它（单向） */
const REACT_PKGS = new Set([
  "react",
  "react-dom",
  "scheduler",
  "react-is",
  "js-tokens",
  "loose-envify",
  "react-refresh",
  "use-sync-external-store",
  "zustand",
]);

/** antd 系：antd 本体 + 其内部件（rc 系 / @rc-component 系）与配套运行时 */
const ANTD_PKG_PREFIXES = ["@ant-design/", "@rc-component/", "rc-", "@babel/", "@emotion/"];
const ANTD_PKGS = new Set([
  "antd",
  "classnames",
  "clsx",
  "dayjs",
  "stylis",
  "json2mq",
  "string-convert",
  "throttle-debounce",
  "compute-scroll-into-view",
  "scroll-into-view-if-needed",
  "is-mobile",
]);

/** markdown 渲染栈（react-markdown / remark-gfm + unified 生态）：仅聊天页按需加载 */
const MARKDOWN_PKG_PREFIXES = [
  "remark-",
  "rehype-",
  "mdast-",
  "micromark",
  "unist-",
  "hast-",
  "unified",
  "vfile",
  "bail",
  "trough",
  "devlop",
  "zwitch",
  "ccount",
  "character-entities",
  "character-reference-invalid",
  "decode-named-character-reference",
  "html-url-attributes",
  "is-plain-obj",
  "longest-streak",
  "markdown-table",
  "normalize-uri",
  "parse-entities",
  "stringify-entities",
  "property-information",
  "space-separated-tokens",
  "comma-separated-tokens",
  "web-namespaces",
  "estree-util-",
  "style-to-",
  "trim-lines",
  "@ungap/",
];
const MARKDOWN_PKGS = new Set(["react-markdown", "remark-gfm", "extend", "escape-string-regexp", "dequal", "inline-style-parser"]);

/** 从模块路径提取包名（兼容 npm 扁平布局与 pnpm 的 .pnpm 嵌套布局） */
function packageNameOf(id: string): string {
  const afterLastNm = id.split("node_modules/").pop() ?? "";
  const segs = afterLastNm.split("/");
  return segs[0]?.startsWith("@") ? segs.slice(0, 2).join("/") : (segs[0] ?? "");
}

/** vendor 分组：按包名确定性分类。按依赖方向 react → antd/markdown 排序，避免 chunk 间循环引用 */
function manualChunks(id: string): string | undefined {
  if (!id.includes("node_modules/")) return undefined;
  const pkg = packageNameOf(id);
  if (REACT_PKGS.has(pkg)) return "vendor-react";
  if (ANTD_PKGS.has(pkg) || ANTD_PKG_PREFIXES.some((p) => pkg.startsWith(p))) return "vendor-antd";
  if (MARKDOWN_PKGS.has(pkg) || MARKDOWN_PKG_PREFIXES.some((p) => pkg.startsWith(p))) return "vendor-markdown";
  // 其余未知三方兜底到独立 chunk，不与业务代码混包
  return "vendor";
}

export default defineConfig({
  plugins: [react()],
  server: {
    port: 5173,
    proxy: {
      "/api": { target: "http://127.0.0.1:4173", changeOrigin: true },
      "/ws": { target: "ws://127.0.0.1:4173", ws: true },
    },
  },
  build: {
    outDir: "dist",
    // antd 全量组件的 vendor chunk 体积固有 >500kB（稳定缓存、与业务代码无关），放宽告警阈值
    chunkSizeWarningLimit: 1500,
    rollupOptions: {
      output: {
        /* UI 迭代只更新入口/视图 chunk，vendor 内容 hash 稳定 → 浏览器长缓存持续命中 */
        manualChunks,
      },
    },
  },
});
