---
tasks: [01M59KY5FRH6P13NZEH9QMCPBA]
---
# 機構アブレーション（専門分担 M1-specialization）の再現手順と hash 確認

選択は測定前に固定済み。選択理由・判断パケット・最終報告は KB 候補にあり、この文書には置かない。ここは再現・確認の手順だけを書く。

## 状態（区別して読む）
- 実装済み: variant 配線（`crates/campaign` の contract / `ablation_application`）、固定 selection。
- 無 LLM 検証済み: 実 `celeris` binary + 実 dispatcher + スタブ worker による通常 HTTP POST の適用試験。**実 pilot ではない**（実 LLM・本番 daemon は使わない）。
- 実 pilot 未実施 / 効果は未測定: 本測定は開始していない。主比較 task とは独立で、止めない。
- 任意延期: 内部レビュー（`internal-review`）、KB 共有（`shared-kb`）。
- 承認待ち: 実 LLM を伴う実 pilot は、残予算の通常読取確認と人の承認の後に、人が operator として行う。

## 比較 ID
`M1-specialization`: `M1-spec-on`（B0 と同じ組織）/ `M1-spec-off`（単一の作業課に畳む）。主比較（A0/B0 系）とは別 ID・別指標で、依存を持たない。

## 1. 固定設定の hash 確認
```sh
sha256sum crates/campaign/fixtures/ablation/selection.sha256 >/dev/null
python3 -I -c "
import json,hashlib
d=json.load(open('crates/campaign/fixtures/ablation/selection.json'))
print(hashlib.sha256(json.dumps(d,sort_keys=True,separators=(',',':'),ensure_ascii=False).encode()).hexdigest())"
cat crates/campaign/fixtures/ablation/selection.sha256
```
2 つが一致すること（固定値 `b6132a4456d00be83fa03bf3a54deaaec5b9874fa7bc0397c0d8bdd8937964bc`）。試験 `selection_is_fixed_and_matches_its_hash` も同じ照合をする。

## 2. 契約・適用試験の再実行
```sh
cargo test -p celeris-campaign --test ablation_contract --test ablation_application
```
期待: ablation_contract 17 passed、ablation_application 4 passed（exit 0）。`CARGO_TARGET_DIR` は Celeris が渡した値のまま使う。

## 3. 証拠の取得
`ablation_application` は一時 DB・token・org・workspace を所有する daemon に通常 POST を送り、次を events / runs から読んで assert する。
- spec-off: 全 task が単一 `coding` 課。spec-on: `coding` と `coding-spec` の 2 課（子・孫を含む）。
- 親子関係、孫の retry（error→done）と対応する run、スタブ PID・run id・events の対応。
- variant 値（`M1-spec-on` / `M1-spec-off`）、selection hash、task / run ID。
- 非対象不変（review・安全上限・全役割モデル・資料・予算・終了条件・grader）を B0 の `BaselinePin` と照合。改変・欠落・取り違えは拒否される。
- 内部レビューは選択していないため、reviewer run 0 / on の assert は対象外（延期）。

試験出力は `cargo test ... -- --nocapture` で取る。試験が SIGKILL されると fixture が `$TMPDIR` に残るので削除する。

## 4. operator 作業（人が行う。未準備なら未準備とする）
実 pilot は本 task の範囲外。実施する場合は `docs/ops/measure-run-final.md` の手順に従い、残予算を通常の読取で確認してから行う。この確認なしに実 LLM を起動しない。
