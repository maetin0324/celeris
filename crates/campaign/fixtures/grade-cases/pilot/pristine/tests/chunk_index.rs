// 実装 problem の受入試験（cargo test --test chunk_index）。grade は grader 側の pristine 写しを重ねて走らせる。
use chunk_index_fixture::chunk_index::chunk_of;

#[test]
fn chunk_zero() {
    assert_eq!(chunk_of(0, 4096), 0);
}

#[test]
fn chunk_last_in_first() {
    assert_eq!(chunk_of(4095, 4096), 0);
}

#[test]
fn chunk_second() {
    assert_eq!(chunk_of(4096, 4096), 1);
}
