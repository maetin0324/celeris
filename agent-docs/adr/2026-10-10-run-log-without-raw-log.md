# ADR: 生ログを保存しない run の表示

- Date: 2026-10-10
- Status: Accepted

## Context

ADR-0078 により browser run は機密保護のため harness の stdout/stderr を保存しない。run 一覧の `files.stdout=false` はこの設計を表す。従来の web run ログ画面はこの値を確認せず stdout file を取得し、404 をログ取得エラーとして表示していた。

## Decision

run 詳細の `files.stdout === false` が確定した場合、web は stdout を要求しない。画面には生ログを保存しない理由を通知し、run の `worker_progress` events（msg、kind、tool、summary、error）を表示する。実行中は events を控えめに再取得する。`files.result === true` の場合は result endpoint の summary と question を表示する。すべての値は React text として描画する。

files が未取得、run が一覧にない、または files が null の間は従来の stdout 表示・取得動作を保つ。task 概要の run リンクは生ログが無いとき「進捗を開く」と案内し、同じ run 画面へ遷移する。

## Consequences

- browser run のログ画面で、保存されない生ログを要求せず、利用可能な進捗と結果を提示できる。
- stdout がある run の既存ログ・復旧動作は変わらない。
