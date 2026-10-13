# web-list: ボード・一覧の待ち理由

- task: 01M4NAKDCCS5XYAAJ518MVZ134
- 実装: `web/features/tasks/wait-reason.tsx` に共有表示部品と理由文言・リンク先を集約。ボードとタスク一覧に理由・遷移先・走れる印を表示し、走れる/待ちの絞り込みを追加。
- モバイル: 既存テーブルの折り返しと最小幅クラスを使い、390px で横溢れしないことを E2E で確認。
- 検証: 下記 result.json の evidence を参照。
