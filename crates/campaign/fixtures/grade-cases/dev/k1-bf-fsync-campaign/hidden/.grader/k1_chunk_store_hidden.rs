// grader 側の隠しテスト（src/storage/chunk_store.rs の子 module として挿入）。出典: 086c4618 の chunk_store.rs tests。
use super::*;
use pluvio_runtime::executor::Runtime;

fn run_test<F>(test: F)
where
    F: std::future::Future<Output = ()> + 'static,
{
    let runtime = Runtime::new(256);
    runtime
        .clone()
        .run_with_name_and_runtime("chunk_store_test", test);
}

#[test]
fn test_posix_explicit_fsync_reads_back() {
    run_test(async {
        let dir = tempfile::tempdir().unwrap();
        let store = PosixChunkStore::new(dir.path(), false).unwrap();
        let data = b"posix fsync regression";
        store.write_chunk("/durable", 0, 0, data).await.unwrap();
        store.fsync_chunk("/durable", 0).await.unwrap();
        assert_eq!(store.read_chunk("/durable", 0, 0, data.len() as u64).await.unwrap(), data);
    });
}

#[test]
fn test_posix_size_xattr_survives_store_reopen() {
    run_test(async {
        let dir = tempfile::tempdir().unwrap();
        let store = PosixChunkStore::new(dir.path(), false).unwrap();
        store.write_chunk("/durable", 0, 0, b"data").await.unwrap();
        store.set_file_size_xattr("/durable", 4).unwrap();
        drop(store);
        let reopened = PosixChunkStore::new(dir.path(), false).unwrap();
        assert_eq!(reopened.get_file_size_xattr("/durable").unwrap(), Some(4));
    });
}
