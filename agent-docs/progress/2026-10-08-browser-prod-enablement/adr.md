---
task: browser-prod-enablement
wu: adr
status: done
completed: 2026-10-08
tasks: [01M4CDNAYX6J68WTX7SKF0DJ64]
---

# adr: ADR 2026-10-08-browser-prod-enablement を書く

## したこと
- `agent-docs/adr/2026-10-08-browser-prod-enablement.md` を追加（D1 台帳の生成・配置・渡し方・古さ判定、D2 前提 gate、D3 site policy の DB 正本と grant の credential API、D4 task policy の自動付与と retry 引き継ぎ、D5 `celerisctl browser doctor`、試験接頭辞 5 つ、セキュリティ方針は変えないと明記）。
- `docs/architecture-map.md` に索引 1 行。
- コードは書いていない（crates/・web/・scripts/ の差分なし）。

## 要点
- 実 LLM は台帳に不要（protocol-scripted + 実 agent-browser 0.38.1 + P4-B 実証拠）。実 LLM での manaba 実機確認は人の手順（docs/ops）。
- 配置は release dir `releases/<sha12>/browser/conformance.json`。daemon は env 上書き → release dir の順で解き、`configure_conformance` で worker に渡す。

## 証拠
- `git diff --name-only $CELERIS_WU_BASE -- crates web scripts` → 空。

## 未解決・提案
- migration 番号は site-policy-api 葉で全ブランチ走査して決める。
- launcher の「害の無い問い合わせ」が既存 IPC にあるかは preflight 葉で確認（無ければ接続のみ）。
