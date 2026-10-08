/**
 * i18n 核心：语言注册表 + 类型安全词典。
 *
 * ── 新增一门语言的步骤 ──
 *  1. 新建 `langs/<code>.ts`：`export const xx = { ... } as const;`（遗漏的 key 自动回退中文）；
 *  2. 在下方 `LANGS` 注册一行 `{ code, name, dict: xx }`；
 *  3. 完成 —— 侧栏语言切换器与持久化自动跟随，无需改任何组件。
 *
 * 约定：
 *  - `langs/zh.ts` 是 key 全集与中文基准；`TKey` 由它导出，拼错 key 编译期报错。
 *  - 词条用扁平点分 key（"nav.chat"）；带参数用 {name} 占位，t("x.hello", { name: "A" })。
 *  - 未收录 key 原样返回（后端下发文案可直接透传）。
 */
import { create } from "zustand";
import { zh } from "./langs/zh";
import { en } from "./langs/en";

/** 语言定义。name 是该语言的母语自称（切换菜单里展示用它，如「日本語」） */
export interface LangDef {
  code: string;
  name: string;
  dict: Partial<Dict>;
}

/** 词典形状：以 zh 为基准。en 等其余语言 Partial，缺失回退 zh */
export type Dict = Record<TKey, string>;
export type TKey = keyof typeof zh;

/** 注册表：新增语言在此加一行 */
export const LANGS: LangDef[] = [
  { code: "zh", name: "中文", dict: zh },
  { code: "en", name: "English", dict: en },
];

const DEFAULT_LANG = "zh";
const LS_KEY = "exm.lang";

function initialLang(): string {
  const saved = localStorage.getItem(LS_KEY);
  if (saved && LANGS.some((l) => l.code === saved)) return saved;
  return DEFAULT_LANG;
}

interface LangState {
  lang: string;
  setLang: (code: string) => void;
}

export const useLang = create<LangState>((set) => ({
  lang: initialLang(),
  setLang: (code) => {
    if (!LANGS.some((l) => l.code === code)) return;
    localStorage.setItem(LS_KEY, code);
    document.documentElement.lang = code; // 字体选择 / 无障碍跟随
    set({ lang: code });
  },
}));

/** 允许动态 key（如后端 schema 字段名）：类型 key 享受检查，动态串运行时回退原样 */
export type TArg = TKey | (string & {});
export type TParams = Record<string, string | number>;

function lookup(lang: string, key: string): string | undefined {
  const l = LANGS.find((x) => x.code === lang);
  return (l?.dict as Record<string, string | undefined>)[key] ?? (zh as Record<string, string | undefined>)[key]; // 目标语缺失 → 回退中文
}

/** 翻译函数。未收录 key 原样返回；{param} 占位替换 */
export function useT(): (key: TArg, params?: TParams) => string {
  const lang = useLang((s) => s.lang);
  return (key, params) => {
    let s = lookup(lang, key) ?? key;
    if (params) {
      for (const [k, v] of Object.entries(params)) s = s.replaceAll(`{${k}}`, String(v));
    }
    return s;
  };
}

/** 非 React 上下文用（store / 工具函数 / 渲染期辅助函数）：同步读当前语言做翻译。
 *  ⚠️ 必须走 `getState()` 而不是 hook：`useLang((s)=>…)` 在组件渲染期间被调用
 *  （如构造 Select options 时）会把 hook 计进当前组件 → 两次渲染间钩子数漂移 → React #310 崩树。
 *  代价：不随语言切换自动重渲染——渲染期文案请用 useT()。 */
export function tr(key: TArg, params?: TParams): string {
  const lang = useLang.getState().lang;
  let s = lookup(lang, key) ?? key;
  if (params) {
    for (const [k, v] of Object.entries(params)) s = s.replaceAll(`{${k}}`, String(v));
  }
  return s;
}
