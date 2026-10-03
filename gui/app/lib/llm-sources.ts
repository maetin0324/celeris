/**
 * `/accounts` の「LLM source」節（ADR-0053 D4、Phase 66）と `/clusters` のトンネル表示で使う純関数。
 * celeris が返した値（`LlmSourcesView` / `ClusterForwardView`）をそのまま表示用に整形するだけで、
 * 判断（到達性・残量・cooldown の計算）はしない（celeris が既に計算済み。ADR-0055 D2 と同じ規律）。
 */

import type { LlmSourceAccountView, LlmSourceView } from "~/celeris/types";
import { formatDuration } from "./time-delta";

/**
 * 供給元 id（`claude-oauth` / `codex-oauth` / `openai-compatible:<id>`）を人が読む見出しにする。
 * `openai-compatible:<id>` は `<id>` だけを出す（設定した名前をそのまま見せる）。
 */
export function sourceLabel(id: string): string {
  if (id === "claude-oauth") return "Claude";
  if (id === "codex-oauth") return "Codex";
  const relay = id.startsWith("openai-compatible:") ? id.slice("openai-compatible:".length) : null;
  return relay && relay.length > 0 ? relay : id;
}

/**
 * 状態バッジの一語（ADR-0055 D2 D1-3: 空白なし、12 字以内）。
 * `openai-compatible` は probe した到達性（`reachable`）、oauth のプールは `enabled` だけを見る
 * （到達性ではなくアカウントの残量で見る供給元なので、`reachable` は元から無い。`docs/api/v1/gui-api.md` §3.108）。
 */
export function sourceStatusWord(source: Pick<LlmSourceView, "enabled" | "reachable">): string {
  if (!source.enabled) return "disabled";
  if (source.reachable === true) return "reachable";
  if (source.reachable === false) return "unreachable";
  return "enabled";
}

/** 0.0〜1.0 を % 表示に（測れないときは「不明」。値を捏造しない。ADR-0024 D3 と同じ規律）。 */
export function formatRemaining(value: number | null | undefined): string {
  if (value == null || !Number.isFinite(value)) return "不明";
  return `${Math.round(Math.min(1, Math.max(0, value)) * 100)}%`;
}

/** `celeris/<tier>` の解決先を人が読む形に（解決先が無ければ「供給元なし」）。 */
export function tierResolutionLabel(resolvesTo: string | null | undefined): string {
  if (!resolvesTo) return "供給元なし";
  return sourceLabel(resolvesTo);
}

/**
 * `celeris/<tier>` が今の供給元に解決した理由の一語（ADR-0055 ラウンド 11、`/accounts` の
 * 「LLM source」節）。**celeris の選択スコアを GUI 側で再計算しない**（gui/CLAUDE.md の禁止事項）:
 * ADR-0053 D1 と ADR-0132 D3 が文書化した規則（cheap でだけ (a) 到達可能な無料源（Qwen 等）を優先 →
 * (b) アカウントプールの残量。frontier / standard は Qwen を候補にしない）を、`GET /llm/sources` が
 * 既に返している値（`kind`・`enabled`・`reachable`・`accounts[].cooldown_until`）だけを読んで説明する。
 * - 解決先が無ければ `"no-source"`。
 * - cheap で解決先が `openai-compatible`（無料中継。Qwen 等）なら `"free-first"`（cheap の優先どおり）。
 *   frontier / standard で `openai-compatible` に解決したと報告されたら、それは規則の外なので
 *   `"unknown"` にする（Qwen 優先とは書かない）。
 * - cheap で解決先が oauth プールで、`openai-compatible` の供給元が設定されていて `reachable === false`
 *   なら `"unreachable"`（Qwen が落ちているので Claude / GPT の cheap に倒れた）。frontier / standard
 *   はそもそも Qwen を候補にしないので、Qwen の到達性を理由にしない。
 * - 解決先が oauth プールで、そのプール内に cooldown 中のアカウントが 1 つでもあれば `"cooldown"`
 *   （どのアカウントが実際に選ばれたかは celeris だけが知っている。「このプールに cooldown 中の
 *   アカウントがいる」という既存の事実を示すだけで、選択の順位までは再現しない）。
 * - それ以外は `"unknown"`（無料源が無い・cooldown も無い、普通のプール選択）。
 */
export type TierResolutionReason = "free-first" | "cooldown" | "unreachable" | "no-source" | "unknown";

export function tierResolutionReason(
  tier: string,
  resolvesTo: string | null | undefined,
  sources: readonly Pick<LlmSourceView, "id" | "kind" | "enabled" | "reachable" | "accounts">[],
  nowSec: number,
): TierResolutionReason {
  if (!resolvesTo) return "no-source";
  const resolved = sources.find((s) => s.id === resolvesTo);
  if (!resolved) return "unknown";
  const cheap = tier === "cheap";
  if (resolved.kind === "openai-compatible") return cheap ? "free-first" : "unknown";
  if (cheap) {
    const freeSource = sources.find((s) => s.kind === "openai-compatible");
    if (freeSource?.enabled && freeSource.reachable === false) return "unreachable";
  }
  if (resolved.accounts.some((a) => isAccountCoolingDown(a, nowSec))) return "cooldown";
  return "unknown";
}

const TIER_RESOLUTION_REASON_LABEL: Record<TierResolutionReason, string> = {
  "free-first": "cheap では無料の Qwen が使えるため優先しています",
  cooldown: "一部アカウントが cooldown 中です",
  unreachable: "cheap の Qwen が届かないため Claude / GPT の cheap に倒れています",
  "no-source": "使える供給元がありません",
  unknown: "通常のアカウント選択です",
};

/**
 * Claude のアカウントが全員 cooldown 中のときの倒れ先の説明（ADR-0132 D3）。Qwen へ倒れるのは
 * `celeris/cheap` だけで、`celeris/frontier`・`celeris/standard` は Codex の GPT にだけ倒れる。
 */
export const CLAUDE_ALL_COOLDOWN_FALLBACK_NOTE =
  "celeris/frontier と celeris/standard は Codex の GPT に倒れます。Qwen を使うのは celeris/cheap だけで、" +
  "Qwen が届いていれば Qwen に、届いていなければ Codex の GPT の cheap に倒れます。";

/**
 * `/llm/sources` の `kind`（`claude-oauth` / `codex-oauth` / `openai-compatible`）を、providers 画面の
 * 「LLM source」節の種類の一語にする（未知の値はそのまま）。
 */
export function sourceKindLabel(kind: string): string {
  const known: Record<string, string> = {
    "claude-oauth": "Claude OAuth",
    "codex-oauth": "Codex OAuth",
    "openai-compatible": "OpenAI 互換",
  };
  return known[kind] ?? kind;
}

/**
 * LLM source が今どの `celeris/<tier>` の解決先になっているか（`celeris_tiers[].resolves_to` を
 * 引き当てるだけ。順は celeris が返した順のまま）。
 */
export function tiersResolvingTo(
  sourceId: string,
  celerisTiers: readonly { tier: string; resolves_to?: string | null }[] | null | undefined,
): string[] {
  return (celerisTiers ?? []).filter((t) => t.resolves_to === sourceId).map((t) => t.tier);
}

/**
 * `openai-compatible`（Qwen 等）の供給元に添える注記（ADR-0132 D3）。oauth のプールには何も付けない。
 */
export function sourceTierScopeNote(kind: string): string | null {
  return kind === "openai-compatible" ? "celeris/cheap にだけ使われます（frontier / standard には使いません）" : null;
}

/**
 * provider（adapter / harness の実行枠）の `llm_source` 参照（ADR-0132 D1・D6。`claude_oauth` /
 * `codex_oauth` / `openai_compatible:<id>` / `celeris` / `none` / `unknown`）を表示用にする。
 * - `celeris` は proxy の `celeris/<tier>` で、**実行時に proxy が供給元を選ぶ**（固定の Qwen ではない）。
 * - `openai_compatible:<id>` だけが特定の OpenAI 互換源（Qwen 等）に直結する。
 * - 参照が無い（古い celeris）・`unknown` は「不明」と出し、Qwen と決めつけない。
 * `sourceId` は `GET /llm/sources` の `sources[].id` の形（突き合わせ用。無ければ `null`）。
 */
export interface ProviderLlmSourceDisplay {
  label: string;
  note: string | null;
  origin: "explicit" | "derived" | null;
  sourceId: string | null;
}

export function providerLlmSourceDisplay(
  ref: { source: string; origin: "explicit" | "derived" } | null | undefined,
): ProviderLlmSourceDisplay {
  if (!ref) return { label: "不明", note: "この celeris は llm_source を返していません", origin: null, sourceId: null };
  const origin = ref.origin;
  const source = ref.source;
  if (source === "celeris") {
    return {
      label: "celeris/<tier>（proxy）",
      note: "実行時に proxy が tier ごとに供給元を選びます（Qwen を使うのは cheap だけ）",
      origin,
      sourceId: null,
    };
  }
  if (source === "claude_oauth") return { label: "Claude OAuth", note: null, origin, sourceId: "claude-oauth" };
  if (source === "codex_oauth") return { label: "Codex OAuth", note: null, origin, sourceId: "codex-oauth" };
  if (source === "none") return { label: "なし", note: "LLM を使わない実行枠です", origin, sourceId: null };
  if (source.startsWith("openai_compatible:")) {
    const id = source.slice("openai_compatible:".length);
    return {
      label: `OpenAI 互換: ${id}`,
      note: "この供給元に直結します（Qwen 直結の実行枠は cheap だけ）",
      origin,
      sourceId: `openai-compatible:${id}`,
    };
  }
  return { label: "不明", note: "設定から供給元を推定できません", origin, sourceId: null };
}

/** `ResolvedLlmSource.origin` を一語に（明示は設定の `llm_source`、導出は旧設定からの推定）。 */
export function llmSourceOriginLabel(origin: "explicit" | "derived" | null): string {
  if (origin === "explicit") return "設定で明示";
  if (origin === "derived") return "旧設定から推定";
  return "";
}

/** `tierResolutionReason` を一行の日本語にする。 */
export function tierResolutionReasonLabel(reason: TierResolutionReason): string {
  return TIER_RESOLUTION_REASON_LABEL[reason];
}

/** `"frontier"` / `"standard"` / `"cheap"` を画面の見出しに（未知の値はそのまま）。 */
export function tierLabel(tier: string): string {
  const known: Record<string, string> = { frontier: "frontier", standard: "standard", cheap: "cheap" };
  return known[tier] ?? tier;
}

/**
 * Unix 秒の `cooldown_until` を、`nowSec`（`fetchedAt` の秒）から見た残り時間に（celeris の
 * `AccountCooldownView.until` は RFC 3339 だが、`LlmSourceAccountView.cooldown_until` は Unix 秒
 * なので変換が要る。celeris の側の型はそのまま、GUI 側だけの表示変換）。
 */
export function cooldownRemainingLabel(cooldownUntilSec: number, nowSec: number): string {
  const remaining = cooldownUntilSec - nowSec;
  if (remaining <= 0) return "切れています";
  return formatDuration(remaining);
}

/** アカウント 1 件が cooldown 中か（`cooldown_until` が未来か）。 */
export function isAccountCoolingDown(account: Pick<LlmSourceAccountView, "cooldown_until">, nowSec: number): boolean {
  return account.cooldown_until != null && account.cooldown_until > nowSec;
}

/**
 * cooldown の絶対時刻（`title` 属性用の RFC 3339。ADR-0055 D2「相対時刻＋`title` に絶対時刻」の規律を
 * Unix 秒の `cooldown_until` にもそのまま適用する）。値を捏造しない: 呼び出し側は
 * `isAccountCoolingDown`/`cooldown_until != null` を確認してから使うこと。
 */
export function cooldownUntilTitle(cooldownUntilSec: number): string {
  return new Date(cooldownUntilSec * 1000).toISOString();
}

/**
 * ある供給元のアカウントが 1 件以上あり、**全員**が cooldown 中か（`/accounts` の「Claude のアカウントが
 * 全部 cooldown 中」の警告カード用）。0 件（アカウントが無い）は「全員 cooldown 中」とは言えないので false。
 */
export function allAccountsCoolingDown(
  accounts: readonly Pick<LlmSourceAccountView, "cooldown_until">[],
  nowSec: number,
): boolean {
  return accounts.length > 0 && accounts.every((a) => isAccountCoolingDown(a, nowSec));
}

/**
 * `/clusters` の port forward（ADR-0053 D3、Phase 66。listener/target の分離は Phase 85）1 本の
 * 状態バッジの一語。`up` が `null`（まだ観測が無い。celeris 再起動直後など）は「unknown」にする
 * （値を捏造しない）。`listener === true` かつ `target_healthy === false` は「転送はあるが先方が
 * 応答しない」（celeris は再発行しない。ADR-0053 Phase 85）ので、`down`（転送自体が無い）とは
 * 別の一語 `unreachable` にする。
 */
export function forwardStatusWord(forward: {
  up?: boolean | null;
  listener?: boolean | null;
  target_healthy?: boolean | null;
}): string {
  if (forward.up === true) return "up";
  if (forward.listener === true && forward.target_healthy === false) return "unreachable";
  if (forward.up === false) return "down";
  return "unknown";
}
