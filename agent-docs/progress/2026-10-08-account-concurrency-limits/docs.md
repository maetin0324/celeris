---
title: providers ガイドと移行手順（本番 config から concurrency を外す人の手順と戻し方）
tasks: [01M4D7RVKXY9A967ZXDADWVHQJ]
status: done
updated: 2026-10-08
---
# docs: providers ガイドと移行手順（本番 config から concurrency を外す人の手順と戻し方）

2026-10-08、WorkUnit `docs`（run 01M4DT63YW895XW259VA1QF7Y1）。ADR 2026-10-08-account-concurrency-limits。完了。

## 変更

- `docs/guides/providers.md`: 設定例から `concurrency` を外し、新しい節「同時実行数はアカウントで決める」を足した
  （`[[providers]].concurrency` は deprecated・警告文・`PATCH /api/v1/accounts/{adapter}/{id}`・実効上限の決まり方と `max_runs_source`・
  `max_runs_per_account` は既定値・暗黙アカウント・CoS +1 は実アカウントだけ・`in_use` は観測値）。旧「CoS の対話 run は枠の外」の段落はこの節に統合した。
- `docs/ops/provider-llm-source-migration.md` の節「プロバイダの concurrency を外し、同時実行数をアカウントで管理する」（scripts/ を触らないため新規 file にせず既存の移行手順に追記）: 人が実行する手順
  （控え → `GET /accounts` で暗黙アカウントの `legacy_concurrency` の引き継ぎを確認 → アカウント画面 / PATCH で上限を保存 →
  config と providers.d から concurrency を外し `celerisctl config to-harnesses` で検査 → `POST /api/v1/reload`（または再起動）→
  `GET /accounts` の実効上限・`journalctl` の警告が 0 件・`GET /providers` の観測値で確認）と戻し方（`max_runs: null`・config の控えを戻す・リリースごと戻す）。
- `docs/ops/provider-llm-source-migration.md`: 設定例から `concurrency` を外し、ローカル優先の表の「同時実行数」を暗黙アカウントに直した。
- `docs/guides/knowledge.md`・`docs/ops/opencode-go-and-model-catalog.md`: 設定例から `concurrency` を外し、上限はアカウントで決まると注記。
- `docs/ops/selfdeploy.md`: verify の smoke 行の表から `concurrency = 1` を外した（`apply_verify_smoke` は `concurrency: None`）。
- `scripts/dev/` は変更しない（attempt 2 で check-doc-links.sh の FOREIGN_DOCS 追加と docs-layout.tsv の行追加を戻した）。

## 検査

| コマンド | exit | 要点 |
| --- | --- | --- |
| `sh scripts/dev/check-doc-links.sh docs agent-docs/progress/2026-10-08-account-concurrency-limits` | 0 | `check-doc-links: ok`（引数なしの全体走査は main 由来の `cos_chat/sink_tests.rs:544` 1 件で exit 1。この WU の範囲外） |
| `sh scripts/dev/check-doc-links.sh --self-test` | 0 | ok |
| `sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` | 0 | ok |
| `sh scripts/dev/check-adr-numbers.sh` | 0 | ok (167 files) |
| `sh scripts/dev/progress-index.sh --check` | 0 | ok |

本番 host・`~/.config/celeris`・systemd・daemon・DB は変更していない。

## 未解決・提案

- `python3 scripts/dev/check-architecture-map.py` は既存の 3 件（`<workspace>/runs/<run_id>/tmp/`・`check-test-tmp-leftovers.sh`・`test-parallel.sh`）で NG。この WU の変更とは無関係。
- `docs/ops/cos-chat.md:87` と `scripts/dev/cos-chat-live.sh` の試験用 provider `concurrency = 4` は、今は効かない（警告が出るだけ）。台本の変更は範囲外なので残した。外すなら別の小さな task で。

## 再確認・補足（2026-10-08、run 01M4DYJ42MQT960PN0ZZ53KN0B）

- 前回の docs 成果を保持し、ADR と実装済みの API・web の表示を照合した。ガイドは deprecated 欄を
  プロバイダの上限として使わないことと、非プール行の移行用既定値として参照することを区別した。
- 移行前の保存値・実効上限の控え、既に保存値がある場合の確認、config を戻してから保存値を復元する順序を補った。
  既定への変更は旧値次第で増減する。暗黙アカウントの UI は実装に合わせて印が付いた行として案内した。
- 人のコマンドは Bash の pipefail・HTTP エラー検出を使い、reload は HTTP 状態と本文を確認する。
  config 検査の変換結果は /dev/null に破棄し、一時 file は作らない。
- 文書リンク（docs 範囲）・レイアウト・リンク検査 self-test・ADR 番号・進捗 index・差分の空白検査は通過。
- 本 run でも CELERIS_WU_BASE は未設定だった。HEAD は先行 WorkUnit の変更を含まないため範囲検査の基点には使わない。
  artifacts/docs-run-base.json に開始時のファイルハッシュを記録し、artifacts/check-docs-run-scope.py で
  本 run の変更が docs/ とこの docs.md だけであること、および crates/ に追加変更が無いことを確認した。
  これは本 run の追加変更の検査であり、harness に unit base を渡す修正の検証ではない。
- 全体のリンク検査には先行の試験コード由来の壊れた参照が残る（別記 artifacts/docs-validation.json）。
  本番 host・設定・systemd・daemon・DB は操作していない。先行 WorkUnit の成果も変更・再 commit していない。

## 計画 check の再確認（2026-10-08、run 01M4DYSHSRM1H28118H2QC1TCJ）

- 前回の検査 3 本を指定された workspace cwd から同じコマンドで実行し、いずれも exit 1 を再現した。
- レイアウト検査は TSV 引数が必須。`scripts/dev/docs-layout.tsv` を渡した形では文書リンク・レイアウトとも exit 0。
- 本 run でも `CELERIS_WU_BASE` は未設定。HEAD は先行工程の成果を含まず、範囲検査が先行成果を拾う。
  未追跡の先行成果もあるため、check 3 の未追跡一覧は基点に含めるか開始前との差分で検査する必要がある。
- 人が承認した check 修正と方針 a（harness による unit base 注入）が未反映なので、done は返さず
  `artifacts/result.json` の `yield.plan_issue` で replan を要求する。詳細と出力は
  artifacts/docs-check-retry-01M4DYSHSRM1H28118H2QC1TCJ.json に記録した。
- 既存の文書・先行工程の成果は保持した。本番 host・設定・systemd・daemon・DB は操作していない。


## 再確認（2026-10-08、run 01M4E0FVHV55248GXTENQY24NA）

- 既存の providers ガイドと移行手順を引き継ぎ、移行手順の設定検査例が一時ファイルを `/tmp` に置かないよう `$TMPDIR` 配下へ修正した。
- 本番 host・設定・systemd・daemon・DB は操作していない。
