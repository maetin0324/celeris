use super::*;

fn table(per_account: Option<u32>, groups: &[(&str, u32)]) -> Arc<ReservationTable> {
    ReservationTable::new(CapacityLimits {
        per_account,
        resource_groups: groups.iter().map(|(g, n)| (g.to_string(), *n)).collect(),
    })
}

#[test]
fn reserve_is_all_or_nothing_and_released_on_drop() {
    let t = table(Some(1), &[("gpu0", 1)]);
    let acct = SlotKey::account("claude-oauth", "a");
    let gpu = SlotKey::group("gpu0");
    let held = t.try_reserve(std::slice::from_ref(&gpu)).unwrap();
    // gpu0 が埋まっているので account も取らない（途中まで取って残さない）。
    let err = t.try_reserve(&[acct.clone(), gpu.clone()]).unwrap_err();
    assert_eq!(err.full, vec![gpu.clone()]);
    assert_eq!(t.held(&acct), 0);
    drop(held);
    assert_eq!(t.held(&gpu), 0);
    let both = t.try_reserve(&[acct.clone(), gpu.clone()]).unwrap();
    assert_eq!(both.slots().len(), 2);
    assert_eq!((t.held(&acct), t.held(&gpu)), (1, 1));
}

#[derive(Debug, Clone, PartialEq)]
struct Pick(&'static str, Vec<SlotKey>);
impl Reservable for Pick {
    fn slots(&self) -> Vec<SlotKey> {
        self.1.clone()
    }
}

#[test]
fn reselect_happens_at_most_once() {
    let t = table(None, &[("gpu0", 1)]);
    let _taken = t.try_reserve(&[SlotKey::group("gpu0")]).unwrap();
    let mut calls = Vec::new();
    // 選択が埋まった枠を返し続けても、選び直しは 1 回で止まる。
    let err = reserve_with_reselect(&t, |attempt, _| {
        calls.push(attempt);
        Some(Pick("a", vec![SlotKey::group("gpu0")]))
    })
    .unwrap_err();
    assert_eq!(calls, vec![0, 1]);
    assert!(matches!(err, ReserveError::Conflict { reselects: 1, .. }));
    assert_eq!(t.peak(&SlotKey::group("gpu0")), 1);
}
