# api-attach: 添付 REST API

- `POST /api/v1/chat/threads/{t}/attachments` は file と client_upload_id を一つずつ受け、chunk ごとに上限を検査する。匿名の一時ファイルは data dir 内の chat/staging に置き、store の `Read` に渡す。store が容量予約、SHA-256、atomic rename と冪等性を管理する。
- GET metadata/content/preview、DELETE、POST references を追加した。content は attachment disposition と nosniff を返す。preview は JPEG/PNG/WebP/GIF を image crate の必要な decoder で読み、40MP を超える画像を拒否し、PNG に再エンコードする。対応外・decode 失敗は 404 と `preview_url=null`。
- 添付 data dir 未設定は 503。API 共通 guard でこの upload route だけ multipart と設定上限に応じた Content-Length を許す。さらに共通 guard が本文ストリーム全体を max_file_bytes + 1 MiB で制限し、file chunk と key 長も実測して制限する。
- 一時 SQLite と data dir に対する `chat_attach_api_*` 試験で、hash/size、上限ちょうどと超過、冪等再送、content header、PNG/非対応/40MP、references と delete を検査する。
