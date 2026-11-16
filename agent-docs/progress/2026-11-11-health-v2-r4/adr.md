# 健全性v2 R4 unit adr: 状態の owner と判断/副作用の分け方の ADR

- 状態: 完了（2026-11-11）。コードは変えていない。
- 成果: [ADR 2026-11-11-dispatcher-state-ownership](../../adr/2026-11-11-dispatcher-state-ownership.md)（D0〜D8）、
  [policy-paths.diff](policy-paths.diff)、[policy-paths.md](policy-paths.md)。

## 広げた範囲（決定 write-set = a）

slices.json は書き換えていない。R4 の範囲に加えたのは、ADR の D1・D8 の owner の module（`crates/task-dispatch/src/` 直下）、
`docs/architecture-map.md` の owner の行の追加と移した行の直し、ADR-0082 の付記だけ。policy_paths の file は加えていない。

## 証拠（提出時の HEAD で実行）

- field → owner の表の行の field 名と `change-map.json` の `fields` 116 件の照合: 欠け 0。ADR の見出しは D0〜D8。
- `git apply --check agent-docs/progress/2026-11-11-health-v2-r4/policy-paths.diff`: exit 0。削除行 0、追加行 43。
- `sh scripts/dev/progress-index.sh --check`・`bash scripts/dev/check-doc-links.sh`・`bash scripts/dev/check-adr-numbers.sh`・
  `python3 scripts/dev/check-human-text.py --root .`: すべて exit 0。

## 未解決事項

- 決定 policy-paths: operator が policy-paths.diff を policy だけの branch で main に入れてから答える（unit sync-policy が待つ）。
- 当てた後の main では arch-map.wrong_owner の orphan-takeover が 1 件増える。own-supervisor が地図に `run_supervisor.rs` の行を足すと消える。

## 提案

- worker_finish の WU 遷移・run index の終了（今は task 遷移より前に別々に commit）を task 遷移と同じ transaction にするかは、R4 では変えない
  （ADR D3: transaction の範囲は今と同じ）。変えるなら別の ADR と反例試験で決める。
