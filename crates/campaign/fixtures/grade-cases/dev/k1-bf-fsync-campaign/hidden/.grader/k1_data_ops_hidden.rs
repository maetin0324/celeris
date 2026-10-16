// grader 側の隠しテスト（src/rpc/data_ops.rs の子 module として挿入）。出典: 086c4618 の data_ops.rs tests。
use super::*;

#[test]
fn fsync_reply_uses_registered_reply_stream() {
    assert_eq!(FsyncChunkRequest::rpc_id(), RPC_FSYNC_CHUNK);
    assert_eq!(FsyncChunkRequest::reply_stream_id(), RPC_FSYNC_CHUNK + 100);
    assert_ne!(FsyncChunkRequest::reply_stream_id(), FsyncChunkRequest::rpc_id());
}

