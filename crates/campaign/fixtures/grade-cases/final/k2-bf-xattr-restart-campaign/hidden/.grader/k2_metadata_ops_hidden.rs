// grader 側の隠しテスト（評価時だけ src/rpc/metadata_ops.rs の子 module として挿入する）。
// 出典: p0-final.patch（provenance は private.json）。公開 repo には置かない。
use super::*;

#[test]
fn lookup_recovers_xattr_after_restart_for_direct_and_by_id_chunks() {
    use crate::rpc::file_id::FileId;
    use crate::rpc::handlers::RpcHandlerContext;
    use crate::storage::chunk_store::PosixChunkStore;
    for by_id in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let path = "/p0-restart";
        let stored = if by_id {
            format!("/benchfs/{:08x}", FileId::compute_path_hash(path))
        } else {
            path.to_string()
        };
        let store = PosixChunkStore::new(dir.path(), false).unwrap();
        let chunk = store.chunk_path(&stored, 0);
        std::fs::create_dir_all(chunk.parent().unwrap()).unwrap();
        std::fs::write(&chunk, b"data").unwrap();
        store.set_file_size_xattr(&stored, 4).unwrap();
        drop(store);
        let mut ctx = RpcHandlerContext::new_bench();
        ctx.chunk_store = Rc::new(PosixChunkStore::new(dir.path(), false).unwrap());
        assert!(!lookup_metadata(&ctx, path, false).is_success());
        let recovered = lookup_metadata(&ctx, path, true);
        assert!(recovered.is_file());
        assert_eq!(recovered.size, 4);
        assert_eq!(ctx.metadata_manager.get_file_metadata(Path::new(path)).unwrap().size, 4);
        assert!(!lookup_metadata(&ctx, "/missing", true).is_success());
    }
}

