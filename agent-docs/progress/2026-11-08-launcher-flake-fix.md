---
title: launcher 試験 a_swapped_daemon_binary… の断続失敗の原因特定と修正
tasks: [01M6VQH1GY87Q79J406DFNQP6G]
status: done
updated: 2026-11-08
---
# launcher 試験の断続失敗

## 結論
本番の tick 修正とは無関係の既存の競合である。`scripts/campaign/measure-launcher.sh` の `owned_alive` が、`/proc/<pid>/cmdline` の一瞬の空を「別 process」と判定していた。直した。

## 原因
- daemon は `nohup` → `sh` の wrapper → 試験 binary と 2 回 exec される。execve の最中は新しい mm の arg_start が決まるまで `/proc/<pid>/cmdline` が空で読める。
- `owned_alive` は starttime が一致しても cmdline に `--config <path>` が無ければ偽を返す。health 待ちの loop がこの瞬間に当たると「daemon exited before becoming healthy」で `cleanup_started` を呼ぶ。`stop_owned` も同じ照合で偽になり、「pid N is not the daemon this launcher started」と出して daemon を止めずに戻る（daemon.log は、まだ何も出力していないので空）。
- 前回の修正 87fce2c7（fork 直後の exec 完了待ち）は最初の exec（nohup）までしか見ておらず、sh → 試験 binary の 2 回目の exec は未対応だった。
- 負荷が高い gate（nextest 4,796 秒）で起きるのは、exec が長く preempt される窓が広がるため。実機では未再現で、症状（メッセージ・daemon.log が空・0.45 秒）との一致と、下の決定的再現で原因とした。

## 変更（commit は件名）
- launcher-flake-fix: owned_alive は starttime が一致する生きた process の空 cmdline を、埋まる・消える・zombie になるまで待つ（上限 500 × 10 ms）。
- 試験専用フック `LAUNCHER_TEST_BLANK_CMDLINE_FILE`（カウンタ file の値だけ owned_alive の cmdline 読みを空にする。本番 env には置かない）と、新試験 `a_transiently_blank_cmdline_during_exec_is_not_taken_for_a_dead_daemon`。負荷も sleep も使わない。

## 証拠
- 修正を外した script で新試験を実行 → 失敗。メッセージは gate と同じ「measure-launcher: pid N is not the daemon this launcher started …」「daemon exited before becoming healthy」、daemon.log は実行前の出力のみ。
- 修正後 `cargo nextest run -p celeris-campaign --test launcher` → 16 passed、1 skipped（対象の a_swapped_daemon… を含む）。
- fmt・clippy・test-affected は提出時の HEAD で実行（下記の完了報告に出力の要点）。

## 本番への移行
config・DB の変更なし。measure-launcher.sh は測定用で、本番 daemon・promote とは無関係。

## 未解決・提案
なし。
