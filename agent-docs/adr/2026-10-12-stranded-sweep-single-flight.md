# ADR: stranded sweep を single-flight にし、変わらない記録で git を呼ばない

- Date: 2026-10-12
- Status: Implemented（実装済み）
- Related: [ADR 2026-10-11-stranded-done-branches](2026-10-11-stranded-done-branches.md)

## Context

`daemon::stranded_sweep::spawn` は 300 秒ごとに `spawn_blocking(run(..))` を無条件に起こしていた。`run` は done の root task（本番で約 1000 件）を毎回すべて見て、repo 参照ごとに `git rev-parse --verify refs/heads/celeris/<id>` を、`task_ops::stranded::sweep` は記録ごとに `git merge-base --is-ancestor <head> refs/heads/<default>` を呼ぶ。既に integrated で head も同じ記録でも呼んでいた。

2026-10-12、repository が遅い NFS 上にあり 1 回の sweep が 300 秒を超え、sweep が重なって積み上がった。`git merge-base --is-ancestor` が 44 本以上 D state で並び、daemon の tick が最大 7014 秒かかり dispatch が数時間止まり、API の task 詳細も応答しなくなった。task 詳細の promotion view も要求ごとに `merge-base --is-ancestor <head> <release_sha>` を呼んでいた。

## Decision

### D1. sweep は single-flight

前回の sweep が走っている間に来た interval の tick は何もせず、前回の経過時間を warn で記録する。占有は drop guard で解放するので、`run` がエラーでも panic でも次の tick は走れる。

### D2. integrated で head が変わらない記録は ancestry を見ない

保存済みの (task, repo) が `integrated` で、branch と head SHA が今回の記録と同じなら `merge-base` を呼ばずに飛ばす。default branch は前にしか進まないので、一度 main の祖先になった head は祖先のままである（force-push で main を巻き戻す運用はしない）。repository・branch・head・default branch の解決に失敗した記録は従来どおり判定する。

### D3. git の読み取りは repository ごとに 1 回

`run` は repository ごとに `git for-each-ref refs/heads/celeris/` を 1 回だけ呼んで head の表を作り、task ごとの `rev-parse` をやめる。default branch の解決も repository ごとに 1 回にする。`task.repos` 無しの task（ADR 2026-10-11 D3）も同じ表を使う。

### D4. promotion view の ancestry は process 内で覚える

`(repository path, branch head, release sha)` に対する `merge-base --is-ancestor` の結果を process 内に覚える。両端が SHA なので結果は変わらない。git が判定を返した場合（exit 0 / 1）だけ覚え、起動できない・object が無いなどの失敗は覚えない。表は上限を超えたら空にする。

## Consequences

- NFS が遅くても sweep は高々 1 本で、tick と API を巻き込んで積み上がらない。
- 定常状態（ほとんどが integrated で head 不変）の sweep の git 呼び出しは repository ごとの数回になる。
- main を巻き戻すと、巻き戻し前に integrated になった記録はその head が変わるまで integrated のまま残る。
