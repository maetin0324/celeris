# task-features schema unit

- ADR `2026-10-11-task-routing-features.md` D1-D4 の `task-core::route_record` API を追加した。
- 固定 schema id: `celeris.task-features/1`, `celeris.route-options/1`, `celeris.route-decision/1`, `celeris.route-outcome/1`; brief template `task-brief/1`; options builder `candidates-one-shot/1`。
- brief は `要求` → `期待する成果物` → `触る範囲` → `置かれた位置` → `過去の試行` → `性質（既存の判定）` の順で作る。自由文は secret 置換後に上限を適用し、超過時は先頭・末尾と省略文字数・sha256 を挿入する。aux は D3(a) の欄を持ち、未取得値は `Option::None` と `missing` で表す。
- secret 規則の選択: task-worker に自由文向け汎用 token/key redactor は無く、局所的な設定 JSON の secret キー置換がある。`task-core` から `task-worker` への依存を作らず、`route_record::redact_secrets` に Bearer、sk-/ghp_/xox* 等の token 形、api_key/token/secret/password の key=value 形を写した。自由文は record 化前にこの関数を通す。
- 学習 export CLI は後続 unit の担当。本 API の features/options は判断 records と評価 manifest の共通入力に使える。CLI の予定形: `celerisctl routing export --db <path> --out <file.jsonl> [--since ...] [--until ...] [--stage ...] [--text brief|none]`（ADR D8）。
- 試験: `cargo test -p task-core route_record_ --lib`（6 tests passed）。`route_record_schema_pinned` は brief・aux・hash と schema/template 定数を固定する。
