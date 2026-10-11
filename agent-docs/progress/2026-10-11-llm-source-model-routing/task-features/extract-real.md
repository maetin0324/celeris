# task-features extract-real

tasks: [01M4NM332S384YYC4VC5DB7KCY]

## 修正

- tasks.json の acceptance を `Criterion { text, check }` として読み、check の type ごとの数を aux に記録。
- WU checks は `WorkUnitCheck { cmd, expect_exit, scope }` として command に正規化。存在しない kind 欄は参照しない。
- retries は work_units.retries 列、repairs は同じ task の repair WU 数を参照。budget は WU を優先し tasks 側へ fallback。
- 公開抽出 API と store 記録 API に `Option<RouteFeatureContext>` を追加。dispatch の既存呼び出しは None を渡す。
- context が指定されない lane/harness/features は、as_of で切った events の seq 順最後の routing_decided から復元する。ctx の値を優先。
- prior run は status / model / wall time と、run id で結んだ失敗 verdict の criterion 文を含める。reason/stdout は保存しない。
- events 読み出しに ORDER BY seq を付与。

## 検証

`cargo test -p task-core route_extract_ --lib` — 成功。実型の JSON fixture で acceptance/check counts、WU retry、repair、budget、brief、ctx 優先を確認。
