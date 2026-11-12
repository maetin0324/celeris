//! 分類段 → dispatcher の受け渡しの board（ADR 2026-10-15-mlsys-min-routing-policy D1/D2、
//! ADR 2026-10-22-jev-classification-before-launch）。`RoutingRuntime` が持ち、読む側（lane 層）は method で引く。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use task_core::route_select::ClassificationEntry;

use crate::CredentialGate;

/// 分類段が置く 1 件。`credential` は鍵未登録・認証失敗のときだけ（secret id と用途。値ではない）。
#[derive(Debug, Clone, PartialEq)]
pub struct BoardItem {
    pub entry: ClassificationEntry,
    pub credential: Option<CredentialGate>,
}

/// board に置かれた 1 回の Jev 呼び出し。`call_id` は呼び出しごとに一意（再起動をまたいでも重ならない）で、
/// 決定ログの消費行の鍵になる。`call_no` は同じ subject の何回目か（2 以上は再分類・再試行）。
#[derive(Debug, Clone, PartialEq)]
pub struct BoardCall {
    pub call_id: String,
    pub call_no: u32,
    pub item: BoardItem,
    /// この呼び出しに結び付いた版メタデータ。従来の `put` は全欄 `None`（欠測）。
    pub provenance: CallProvenance,
}

/// 1 回の Jev 呼び出しの版メタデータ。`call_id` と同じ `BoardCall` に載り、subject だけで引く別 call の値と混ざらない。
/// 欠測は `None`（設定の model 名で埋めない・片方から他方を複製しない）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CallProvenance {
    /// 応答が名乗った model 名。
    pub response_model: Option<String>,
    /// 応答と独立に観測した版。
    pub observed_snapshot: Option<String>,
}

#[derive(Debug, Default)]
struct BoardState {
    current: HashMap<String, BoardCall>,
    /// subject ごとの呼び出し回数（`remove` しても減らさない）。
    calls: HashMap<String, u32>,
    /// まだ決定ログへ書いていない呼び出し（dispatcher が読む前に置き換わった分を落とさない）。
    unrecorded: HashMap<String, Vec<BoardCall>>,
}

/// 分類段 → dispatcher の受け渡し（subject = work unit id、無ければ task id）。dispatcher は読むだけ。
#[derive(Debug, Clone, Default)]
pub struct ClassificationBoard {
    inner: Arc<Mutex<BoardState>>,
}

fn next_call_id() -> String {
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicU64, Ordering};
    static START: OnceLock<u128> = OnceLock::new();
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let start = *START.get_or_init(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos())
    });
    format!("{start:x}-{}", SEQ.fetch_add(1, Ordering::Relaxed))
}

impl ClassificationBoard {
    /// 呼び出し 1 回分を置く。置くたびに新しい `call_id` が付く（前の呼び出しは決定ログに残る）。
    pub fn put(&self, subject: &str, item: BoardItem) {
        self.put_with_provenance(subject, item, CallProvenance::default());
    }

    /// `put` と同じ採番（`call_id`・`call_no`）で、版メタデータを同じ呼び出しに原子的に結び付けて置く。
    pub fn put_with_provenance(&self, subject: &str, item: BoardItem, provenance: CallProvenance) {
        if let Ok(mut s) = self.inner.lock() {
            let n = s.calls.entry(subject.to_owned()).or_insert(0);
            *n += 1;
            let call = BoardCall {
                call_id: next_call_id(),
                call_no: *n,
                item,
                provenance,
            };
            s.unrecorded
                .entry(subject.to_owned())
                .or_default()
                .push(call.clone());
            s.current.insert(subject.to_owned(), call);
        }
    }

    pub fn get(&self, subject: &str) -> Option<BoardItem> {
        self.get_call(subject).map(|c| c.item)
    }

    pub fn get_call(&self, subject: &str) -> Option<BoardCall> {
        self.inner.lock().ok()?.current.get(subject).cloned()
    }

    /// 決定ログへ未記録の呼び出しを古い順に取り出す（取り出したら空になる）。
    pub fn take_unrecorded(&self, subject: &str) -> Vec<BoardCall> {
        self.inner
            .lock()
            .ok()
            .and_then(|mut s| s.unrecorded.remove(subject))
            .unwrap_or_default()
    }

    pub fn remove(&self, subject: &str) {
        if let Ok(mut s) = self.inner.lock() {
            s.current.remove(subject);
        }
    }
}
