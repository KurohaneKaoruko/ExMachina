/** 默认模型选择工具：智能体/智能体组设置共用（选项来自「提供商」档案） */
import type { LlmProfile } from "./api";

export interface ModelOption {
  value: string;
  label: string;
}

/** 默认模型选项：空值 = 跟随全局默认（当前生效提供商）；其余 = 档案ID 或 档案ID/模型名 */
export function buildModelOptions(profiles: LlmProfile[]): ModelOption[] {
  const opts: ModelOption[] = [{ value: "", label: "跟随全局默认（当前生效提供商）" }];
  for (const p of profiles) {
    if (p.orchModel) opts.push({ value: p.id, label: `${p.name} · ${p.orchModel}（指挥体）` });
    if (p.unitModel && p.unitModel !== p.orchModel) {
      opts.push({ value: `${p.id}/${p.unitModel}`, label: `${p.name} · ${p.unitModel}（子个体）` });
    }
  }
  return opts;
}

/** 把 modelHint（档案ID 或 档案ID/模型名）解析为可读标签 */
export function modelLabel(hint: string | null | undefined, profiles: LlmProfile[]): string {
  if (!hint) return "跟随全局";
  const slash = hint.indexOf("/");
  const pid = slash > 0 ? hint.slice(0, slash) : hint;
  const explicit = slash > 0 ? hint.slice(slash + 1) : "";
  const p = profiles.find((x) => x.id === pid);
  if (!p) return hint;
  return explicit ? `${p.name} · ${explicit}` : `${p.name} · ${p.orchModel || pid}`;
}
