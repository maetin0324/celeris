# CoS の自由質問への本文回答

- 日付: 2026-10-27
- tasks: [01M5XXF7QECJ78NWVAHDH7A2BF]

## 決定

worker の質問（InboxKind::Question）は選択肢を持つ decision と区別する。受信箱の answer/replan は回答内容の候補ではなく操作の選択である。CoS の resolve と inbox relay、および人の受信箱回答で option を省略でき、note の非空本文を通常の question.answer に渡す。明示した option は引き続き提示された key に限る。replan は明示選択したときだけ発動する。直接の question.answer は既存の answer 本文を使う。

選択肢のある decision・approval・gate の検証は緩めない。存在しない reply 等の key は拒否する。decision の自由記述は既存の domain API が許す choice の option 省略と note を使う。受信箱の option 省略を decision へ一般化しない。

resolve と relay は同じ回答検証を使い、空本文・未知 key を domain 操作前に拒否する。revision、human_required、冪等性、解決済み待ちの拒否、監査付き transaction は維持する。エスカレーションの reply は人への自由記述案内専用で、回答の key として推測しない。

## 根拠と移行

CoS が自由質問にも option を要求されるため、本文があっても誤った key で拒否される。domain の question.answer は既に本文で再開できるため、受信箱の入力制約を合わせる。DB migration と本番設定変更は不要。CoS の配布 skill に省略形式と reply の用途を明記する。
