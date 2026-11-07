"""repo health 検査器の中核（ADR 2026-11-07-repo-health-contract）。

- gitio: git の呼び出しと blob の一括読み出し
- snapshot: base・head の解決、dirty・shallow・submodule、変更 file と変更行
- adapters: 言語と file の種類（本体・試験・生成・fixture・migration・docs）の分類
- profile: profile の解決（cli > repo > builtin）と base 側での読み込み、検査
- rule: rule の登録口（rules/ の module）と finding の素
- engine: rule の評価（base と head の両方）・delta・集約・report の組み立て
"""

TOOL_VERSION = "1.0"
