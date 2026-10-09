---
title: CoS 全 API 変更操作の範囲と監査契約（adr WorkUnit）
tasks: [01M4F24B0GAEVZQPP35830PA0F]
status: done
updated: 2026-10-09
---
# adr WorkUnit

完了日: 2026-10-09。設計のみ。API 実装完了ではない。

[新 ADR](../../adr/2026-10-09-cos-operations-all-mutations.md) に、人の決定、理由付き除外表、全 method/path の
5 領域割当、action 命名規則、OperationAudit の共有操作関数と既存 validation の維持を確定した。
[旧 ADR](../../adr/2026-10-05-cos-chat-home.md) D3 の差分節と関連の close-out/attach-handoff 参照を更新し、
設計判断の解消と後続の実装を区別した。

## 棚卸しの確定値

base `409625f5` の router と API endpoint 表から 142 method/path（共通 128、router のみ 14、文書のみ 0）。
動的な browser control の BASE/format! と site-policies の put_route 別名を展開、引数名の表記差を正規化した。
通常許可 103、読み取り POST の監査対象 3、除外 36（秘密系列 8、browser credential/attestation 14、
console/instruct 1、cos 再帰 4、既存 ADR-0079 の撤去済み 9）。
領域の全行は tasks 22、decisions 11、projects 23、admin 45、surface 41。

## 検証

新規文書を staging した後に実行（2026-10-09）。

| 検査 | コマンド | 結果 |
|---|---|---|
| 成果物・旧 D3 の参照 | `test -f agent-docs/adr/2026-10-09-cos-operations-all-mutations.md` と旧 ADR への slug の `grep -q` | exit 0 |
| 文書リンク | `sh scripts/dev/check-doc-links.sh` | exit 0、check-doc-links: ok |
| ADR 採番 | `sh scripts/dev/check-adr-numbers.sh` | exit 0、169 files |
| progress front matter | `sh scripts/dev/progress-index.sh --check` | exit 0 |
| コード差分無し | `git diff --quiet 409625f5 -- crates/ config/ web/ gui/` | exit 0 |
| 変更範囲 | base からの `git diff --name-only` と untracked の和集合を ADR/progress 配下に限定 | exit 0、対象は 3 文書だけ |
| 表の網羅性 | router/API 表の抽出集合と新 ADR の D3 の集合を Python で比較 | exit 0、142 unique method/path、重複無し、5 領域 |
| whitespace | `git diff --cached --check` | exit 0 |

workspace に repository が無かったため、`/home/rmaeda/workspace/agent-platform` の clean HEAD `409625f5` から
`repos/agent-platform` に独立 clone を作成した。棚卸しの抽出資料は workspace の artifacts にあり、
repository には確定した ADR と progress だけを置いた。
この unit はコードを変更しないため、workspace の clippy/test-parallel は最終 verify unit の仕事である。

## 残る仕事

registry → 5 領域 → ctl-wrap/skill-docs → verify の実装と試験。promote の selfdeploy 文書との整合は ops-admin、
最終 ALLOWED 登録済みの D3 追記は skill-docs/verify が担う。新たな人の決定要求は無い。
本番 host・daemon・DB・元 checkout は変更していない。
