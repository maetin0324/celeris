---
title: K4 ops-r1 pilot 再 pin（repin）
tasks: [01M62S8XWCZDCY2ZBVQ35176MH]
status: done
updated: 2026-10-30
completed: 2026-10-30
---
# K4 ops-r1 pilot 再 pin（repin）

状態: 完了。fix-pipeline を含む celeris-campaign を pin し直し、B の daemon を本番 current の release（固定票の `release.adopted_release`。以下「新 release」）に pin した。有料試行は 0 件。

成果物（repo の外）: `/local/celeris/data/workspaces/01M62S8XWCZDCY2ZBVQ35176MH/artifacts/ops-r1/freeze/`

- `bin/celeris-campaign-repin-<build 時の WU HEAD の 12 字>`（path は固定票の `tool_binaries.celeris-campaign.path`）: 新しい binary（sha256 は固定票の `tool_binaries.celeris-campaign.sha256`）。旧 `bin/celeris-campaign`（固定票の `tool_binaries.previous`）は残した
- `measure/daemon.toml`: `ops-r1-measure-config.sh --release-dir <新 release の dir>` で作り直した（差はコメント行だけ）。旧版の写しは `measure/daemon.94f86928dc13.toml`
- `fixed-ticket.json`・`fixed-ticket.md`: release・tool_binaries・daemon_config を更新し、`revisions` と「改訂履歴」に理由を書いた
- `gap-resolution.json`: `g-ops-r1-pipeline-manifest` を resolved=true で足した（all_resolved true）
- `digests.sha256`: 作り直した（新旧の binary・daemon.toml を含む 8 件）

## 結論

- WU branch の HEAD に fix-pipeline が無かった（統合先 branch の「integrate wu/fix-dagbench-pins (phase fixpipe)」が祖先でない）。統合先 branch `celeris/01M62S8XWCZDCY2ZBVQ35176MH` を一時 merge して build し、build と試験の後に WU branch から merge を外した（範囲の check のため。WU branch は進捗ファイルだけを変える。freeze と同じ扱い）。binary の元の crates/campaign の git tree は統合先 branch と同じ（tree hash は固定票の `built_from`）。
- 本番の current は新 release（2026-10-29T14:48:50Z 昇格、previous は 94f86928dc13）。health の release も新 release、cos_policy_version "6"。読んだだけで書き換えていない。
- 決定 b-env（measure-promoted）に従い、採用 release を 94f86928dc13 → 新 release に替えた。差は 9 commit で、daemon 側に 41 file の差がある。skill の差は cos-inbox-triage v6→v7 だけ。測定 daemon は本番と同じく policy_version "6" を明示し、skill は repo の v6（測定 KB の skills tree の digest は変わらない）。
- measurement_id・condition_id・trial_id は変えない。契約 §3 の新しい condition_id は試行開始後の変更の規則で、試行はまだ 1 件も始まっていない。

## 証拠（build と ops-r1 の試験は一時 merge した tree で実行。基準は `$CELERIS_WU_BASE`）

- `cargo build --release -p celeris-campaign --bin celeris-campaign` → exit 0
- `cargo nextest run -p celeris --test ops_r1_pipeline` → exit 0、2 passed
- `cargo nextest run -p celeris-campaign -E 'test(ops_r1) | test(human_op) | test(conditions)'` → exit 0、36 passed
- `sh scripts/campaign/ops-r1-measure-config.sh ... --release-dir <新 release の dir> ...` → exit 0
- `(cd <freeze> && sha256sum -c digests.sha256)` → exit 0、8 件 OK
- fixed-ticket.json の celeris-campaign の sha256 と bin の実ファイルの sha256 が一致（python で照合）
- 提出時の HEAD（進捗ファイルだけの差）で `bash scripts/dev/test-affected.sh`・`cargo clippy --workspace -- -D warnings`・`cargo fmt --all -- --check`・`sh scripts/dev/progress-index.sh --check` を実行した（結果は報告に書く）

## 未解決

- 新 release の celeris で `measure/daemon.toml` を読めるかは未確認。preflight が daemon を起動して確かめる。
- B の CoS 消費を試行の tokens に含めるか（fix-pipeline の未解決）は人の判断待ちのまま。
- releases/ に未昇格の release が 2 つある（固定票の `release.production_release_observed.newer_unpromoted_dirs`）。preflight より前に本番が替わったら、同じ手順で repin する。

## 提案

- release が試行の前に替わる度に repin が要る。preflight の開始時に health の release と固定票の adopted_release を照合し、違えば止める検査を preflight に入れる。
