/** 默认模型选择工具：智能体/智能体组设置共用（选项来自「提供商」档案） */
import type { LlmProfile } from "./api";
import { tr } from "./i18n/core";

export interface ModelOption {
  value: string;
  label: string;
}

/** 默认模型选项：空值 = 跟随全局默认（当前生效提供商）；其余 = 档案ID（可带 /模型名 覆盖） */
export function buildModelOptions(profiles: LlmProfile[]): ModelOption[] {
  const opts: ModelOption[] = [{ value: "", label: tr("models.followGlobalOption") }];
  for (const p of profiles) {
    opts.push({ value: p.id, label: `${p.name} · ${p.model || tr("models.noModel")}` });
  }
  return opts;
}

/** 能力模型选项（模型设置页 / 组级覆盖共用）：档案默认（value=档案ID）+ 清单逐模型（value=档案ID/模型名） */
export function buildCapabilityOptions(profiles: LlmProfile[]): ModelOption[] {
  const opts: ModelOption[] = [];
  for (const p of profiles) {
    if (p.model) opts.push({ value: p.id, label: `${p.name} · ${tr("models.capDefault")}` });
    for (const m of p.models) {
      opts.push({ value: `${p.id}/${m.model}`, label: `${p.name} / ${m.model}` });
    }
  }
  return opts;
}

/** 把 modelHint（档案ID 或 档案ID/模型名）解析为可读标签 */
export function modelLabel(hint: string | null | undefined, profiles: LlmProfile[]): string {
  if (!hint) return tr("models.followGlobalShort");
  const slash = hint.indexOf("/");
  const pid = slash > 0 ? hint.slice(0, slash) : hint;
  const explicit = slash > 0 ? hint.slice(slash + 1) : "";
  const p = profiles.find((x) => x.id === pid);
  if (!p) return hint;
  return explicit ? `${p.name} · ${explicit}` : `${p.name} · ${p.model || pid}`;
}

/** 组 id 的英文标注（纯展示映射，数据 identifier 不变）：内置默认组用品牌标注 */
export function groupIdEn(id: string): string {
  if (id === "default") return tr("enGroup.default");
  return id.toUpperCase();
}

/** 个体 identifier 的英文标注（纯展示映射）：指挥体去 EXMACHINA- 冗余前缀 */
export function agentIdEn(identifier: string): string {
  if (identifier === "exmachina-orchestrator") return tr("enAgent.orchestrator");
  return identifier.toUpperCase();
}
