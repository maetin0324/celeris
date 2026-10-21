# 段の統合の検査: 子孫ごと止める・log を残す・統合中は lease を延長する

- 日付: 2026-10-20
- 状態: 採用

## 背景
task 01M5C8KF8Y（2026-10-20）で、段の統合の検査 `cargo test ... 2>&1 | tail -20` が sccache の rustc 停止で進まず 1800 s で時間切れになった。cargo は殺されず孤児で残り、後続の検査が build lock を待って 4 回時間切れ、log は `| tail` のため 0 byte、段の統合が lease より長く続いて「lease expired; reclaimed」で task が failed になった。

## 決定
- D1 検査・WU check の process（`LocalWorkspace::exec`）は独立の process group で動く。時間切れでは、先に子孫（ppid を辿る）を数えて、group と子孫を SIGKILL する。`exec` の future が drop（中断）されても同じ guard が group と子孫を撃つ。cgroup / systemd scope は使わない（worker sandbox では使えず、group + 子孫の走査で今回の孤児は足りる）。
- D2 時間切れのとき、止めた子孫の `pid comm state wchan` を `with_output_log` の log の末尾に残す（`wait_woken` の停止が log から分かる）。出力を打ち切りで捨てる `| tail` は、check の側の書き方の問題だが、planner の指示文は変えない（planner が `| tail` を避ける指示は未実装の提案）。出力を直接 log に流す方式は、既に `with_output_log` が stdout/stderr を逐次 file へ書くので採用済み。コマンドが自分で `| tail` を付けた場合も、時間切れの追記で log は 0 byte にならない。
- D3 段の統合が走っている task（`integrating`）の lease 切れは回収せず、lease を延長する。検査は 1 件ごとに時間切れで必ず終わるので無限には続かない。時間切れの繰り返しは既存の `integration_gives_up`（replan 上限後は blocked の質問）に流れる。その理由には `command timed out after Ns: <cmd>` が入る。
- D4 旧い挙動（直接の子だけ `killpg` する分岐）は消した。

## 提案（未実装）
- sccache の rustc 停止に備え、時間切れの理由に「cargo が lock 待ち / 子が停止（wchan）」を出す。stale な孤児を検査の開始前に target dir 単位で掃除する。
