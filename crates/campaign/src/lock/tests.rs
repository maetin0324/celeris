use super::*;
use crate::manifest::fixtures::{campaign, input};

type Edit = Box<dyn Fn(&mut CampaignLock)>;

fn h(c: char) -> String {
    c.to_string().repeat(64)
}
fn g(c: char) -> String {
    c.to_string().repeat(40)
}

fn case(id: &str) -> LockedCase {
    LockedCase {
        id: id.into(),
        split: Split::Dev,
        implementation: true,
        initial: Some(LockedInitial {
            commit: g('a'),
            tree: g('b'),
            content_sha256: h('1'),
            submodules: vec![
                LockedSubmodule {
                    path: "ext/b".into(),
                    commit: g('c'),
                    content_sha256: h('2'),
                },
                LockedSubmodule {
                    path: "ext/a".into(),
                    commit: g('d'),
                    content_sha256: h('3'),
                },
            ],
        }),
        inputs: vec![DeliveredInput {
            name: "data.tgz".into(),
            sha256: h('4'),
        }],
        grader: GraderPin {
            private_sha256: h('5'),
            grade_case_sha256: h('6'),
        },
        tests: vec![
            LockedTest {
                path: "tests/pub.rs".into(),
                visibility: TestVisibility::Public,
                sha256: h('7'),
            },
            LockedTest {
                path: "tests/hid.rs".into(),
                visibility: TestVisibility::Hidden,
                sha256: h('8'),
            },
        ],
    }
}

fn lock() -> (CampaignManifest, CampaignLock) {
    let i = input(2, "dev");
    let mut m = campaign(&i, Split::Dev, 2);
    m.snapshot.materials_sha256 = h('e');
    let l = CampaignLock::new("1", "m-1", &m, vec![case("p1"), case("p0")])
        .expect("new")
        .seal()
        .expect("seal");
    (m, l)
}

fn reseal(mut l: CampaignLock) -> CampaignLock {
    l.sha256 = String::new();
    l.seal().expect("seal")
}

#[test]
fn hash_is_order_insensitive_and_verifies() {
    let (m, l) = lock();
    let mut r = l.clone();
    r.cases.reverse();
    r.cases[0].initial.as_mut().expect("i").submodules.reverse();
    r.cases[1].tests.reverse();
    assert_eq!(r.lock_hash().expect("h"), l.sha256);
    assert!(verify_lock(&l, &reseal(r)).is_ok());
    assert!(l.bind_to_manifest(&m).is_ok());
}

#[test]
fn shared_approval_needs_a_lock_bound_to_the_ledger_declaration() {
    use crate::manifest::{ConfigId, ExperimentSet, SetFixed};
    let (m, legacy) = lock();
    // 旧形式（共有承認の宣言なし）は従来どおり何も見ない。
    assert!(legacy.check_shared_ledger(&m).is_ok());
    let shared = |p: &str| {
        let mut s = m.clone();
        s.experiment_sets.insert(
            "all".into(),
            ExperimentSet {
                arms: vec![ConfigId::A, ConfigId::B, ConfigId::C],
                compare: vec![(ConfigId::A, ConfigId::B), (ConfigId::B, ConfigId::C)],
                fixed: SetFixed {
                    approved_trials: Some(4),
                    shared_ledger: Some(p.into()),
                    ..Default::default()
                },
            },
        );
        s
    };
    let s = shared("/approved/ledger");
    let bound = |m: &CampaignManifest| {
        CampaignLock::new("1", "m-1", m, vec![case("p1"), case("p0")])
            .expect("new")
            .seal()
            .expect("seal")
    };
    assert!(bound(&s).check_shared_ledger(&s).is_ok());
    // 宣言の無い旧 lock は結び付きが不明なので拒否。
    let e = legacy.check_shared_ledger(&s).expect_err("legacy lock");
    assert!(e.contains("does not bind the shared ledger"), "{e}");
    // 別の台帳 path を宣言した manifest の lock は使えない（承認と lock の台帳が食い違う）。
    let e = bound(&shared("/other/ledger"))
        .check_shared_ledger(&s)
        .expect_err("other ledger");
    assert!(e.contains("not bound to this shared approval"), "{e}");
}

#[test]
fn lock_is_bound_to_the_campaign_hash() {
    let (m, l) = lock();
    let mut m2 = m.clone();
    m2.budget.max_tokens += 1;
    assert!(matches!(
        l.bind_to_manifest(&m2),
        Err(LockError::CampaignMismatch(_))
    ));
    let mut l2 = l.clone();
    l2.campaign_sha256 = h('9');
    assert!(matches!(
        verify_lock(&l, &reseal(l2)),
        Err(LockError::CampaignMismatch(_))
    ));
}

#[test]
fn duplicates_and_bad_paths_are_rejected() {
    let (_, l) = lock();
    let mut d = l.clone();
    d.cases[1].id = d.cases[0].id.clone();
    assert!(matches!(d.validate(), Err(LockError::Duplicate(_))));
    let mut d = l.clone();
    let subs = &mut d.cases[0].initial.as_mut().expect("i").submodules;
    subs[1].path = subs[0].path.clone();
    assert!(matches!(d.validate(), Err(LockError::Duplicate(_))));
    let mut d = l.clone();
    d.cases[0].tests[1].path = d.cases[0].tests[0].path.clone();
    assert!(matches!(d.validate(), Err(LockError::Duplicate(_))));
    for bad in ["/abs", "a/../b", "", "a//b"] {
        let mut d = l.clone();
        d.cases[0].initial.as_mut().expect("i").submodules[0].path = bad.into();
        assert!(d.validate().is_err(), "{bad:?}");
    }
}

#[test]
fn unpinned_values_are_rejected() {
    let (_, l) = lock();
    let edits: Vec<Edit> = vec![
        Box::new(|l| l.measurement_id.clear()),
        Box::new(|l| l.version.clear()),
        Box::new(|l| l.materials_sha256.clear()),
        Box::new(|l| l.kb_snapshot.clear()),
        Box::new(|l| l.cases.clear()),
        Box::new(|l| l.cases[0].initial.as_mut().expect("i").tree = String::new()),
        Box::new(|l| l.cases[0].initial.as_mut().expect("i").commit = "abc".into()),
        Box::new(|l| l.cases[0].initial.as_mut().expect("i").content_sha256 = h('A')),
        Box::new(|l| {
            l.cases[0].initial.as_mut().expect("i").submodules[0]
                .content_sha256
                .clear()
        }),
        Box::new(|l| l.cases[0].inputs[0].sha256.clear()),
        Box::new(|l| l.cases[0].grader.grade_case_sha256.clear()),
        Box::new(|l| l.cases[0].tests[0].sha256.clear()),
    ];
    for (n, e) in edits.iter().enumerate() {
        let mut d = l.clone();
        e(&mut d);
        assert!(d.validate().is_err(), "edit {n}");
        assert!(verify_lock(&l, &d).is_err(), "edit {n}");
    }
}

#[test]
fn implementation_case_must_pin_public_and_hidden_tests() {
    let (_, l) = lock();
    // 初期木が無い case は公開テストも個別に固定する。初期木がある case は公開テストを initial の内容 hash が固定する。
    for vis in [TestVisibility::Public, TestVisibility::Hidden] {
        let mut d = l.clone();
        d.cases[0].initial = None;
        d.cases[0].tests.retain(|t| t.visibility != vis);
        assert!(matches!(d.validate(), Err(LockError::MissingTests(_))));
    }
    let mut with_initial = l.clone();
    with_initial.cases[0]
        .tests
        .retain(|t| t.visibility != TestVisibility::Public);
    assert!(with_initial.validate().is_ok());
    with_initial.cases[0].tests.clear();
    assert!(matches!(
        with_initial.validate(),
        Err(LockError::MissingTests(_))
    ));
    let mut d = l.clone();
    d.cases[0].tests.clear();
    assert!(matches!(d.validate(), Err(LockError::MissingTests(_))));
    // 実装でない case は空でよい。
    let mut ok = l.clone();
    ok.cases[0].implementation = false;
    ok.cases[0].tests.clear();
    assert!(ok.validate().is_ok());
}

#[test]
fn version_and_measurement_differences_are_distinguished() {
    let (_, l) = lock();
    let mut v = l.clone();
    v.version = "2".into();
    assert!(matches!(
        verify_lock(&l, &reseal(v)),
        Err(LockError::VersionMismatch { .. })
    ));
    let mut m = l.clone();
    m.measurement_id = "m-2".into();
    assert!(matches!(
        verify_lock(&l, &reseal(m)),
        Err(LockError::MeasurementMismatch { .. })
    ));
}

#[test]
fn same_measurement_with_changed_content_is_rejected_per_kind() {
    let (_, l) = lock();
    let cases: Vec<(&str, Edit)> = vec![
        ("materials", Box::new(|l| l.materials_sha256 = h('9'))),
        ("kb", Box::new(|l| l.kb_snapshot = "other".into())),
        (
            "grader",
            Box::new(|l| l.cases[0].grader.private_sha256 = h('9')),
        ),
        (
            "grade_case",
            Box::new(|l| l.cases[1].grader.grade_case_sha256 = h('9')),
        ),
        (
            "hidden test",
            Box::new(|l| l.cases[0].tests[0].sha256 = h('9')),
        ),
        ("input", Box::new(|l| l.cases[0].inputs[0].sha256 = h('9'))),
        (
            "tree",
            Box::new(|l| l.cases[0].initial.as_mut().expect("i").tree = g('9')),
        ),
        (
            "content",
            Box::new(|l| l.cases[0].initial.as_mut().expect("i").content_sha256 = h('9')),
        ),
        (
            "sub swap",
            Box::new(|l| l.cases[0].initial.as_mut().expect("i").submodules[0].commit = g('9')),
        ),
        (
            "sub content",
            Box::new(|l| {
                l.cases[0].initial.as_mut().expect("i").submodules[1].content_sha256 = h('9')
            }),
        ),
        (
            "sub removed",
            Box::new(|l| {
                l.cases[0].initial.as_mut().expect("i").submodules.pop();
            }),
        ),
        (
            "sub added",
            Box::new(|l| {
                l.cases[0]
                    .initial
                    .as_mut()
                    .expect("i")
                    .submodules
                    .push(LockedSubmodule {
                        path: "x".into(),
                        commit: g('9'),
                        content_sha256: h('9'),
                    })
            }),
        ),
        (
            "case removed",
            Box::new(|l| {
                l.cases.pop();
            }),
        ),
        (
            "case added",
            Box::new(|l| {
                let mut c = case("p9");
                c.id = "p9".into();
                l.cases.push(c);
            }),
        ),
        ("exec", Box::new(|l| l.execution.seed += 1)),
    ];
    for (name, e) in cases {
        let mut d = l.clone();
        e(&mut d);
        let r = verify_lock(&l, &reseal(d));
        assert!(r.is_err(), "{name} accepted");
        assert!(
            !matches!(r, Err(LockError::ContentChanged(_))),
            "{name}: unclassified"
        );
    }
    let mut d = l.clone();
    d.cases[0].initial.as_mut().expect("i").submodules[0].commit = g('9');
    assert!(matches!(
        verify_lock(&l, &reseal(d)),
        Err(LockError::SubmoduleMismatch(_))
    ));
    let mut d = l.clone();
    d.cases[0].tests[0].sha256 = h('9');
    assert!(matches!(
        verify_lock(&l, &reseal(d)),
        Err(LockError::TestMismatch(_))
    ));
}

#[test]
fn tampered_content_without_resealing_is_rejected() {
    let (_, l) = lock();
    let mut d = l.clone();
    d.cases[0].grader.private_sha256 = h('9');
    assert!(matches!(
        verify_lock(&l, &d),
        Err(LockError::SealMismatch(_))
    ));
    // 期待側の改変も拒否（信頼側も再計算で照合する）。
    assert!(matches!(
        verify_lock(&d, &l),
        Err(LockError::SealMismatch(_))
    ));
}

#[test]
fn changed_content_needs_a_new_measurement_id() {
    let (_, l) = lock();
    let mut n = l.clone();
    n.cases[0].grader.private_sha256 = h('9');
    let same_id = reseal(n.clone());
    assert!(matches!(
        check_successor(&l, &same_id),
        Err(LockError::NotSuccessor(_))
    ));
    n.measurement_id = "m-2".into();
    let new = reseal(n);
    assert!(check_successor(&l, &new).is_ok());
    // 旧 lock は新 lock で上書きされず、旧 lock の照合は旧内容を基準にし続ける。
    assert!(verify_lock(&l, &new).is_err());
    assert!(verify_lock(&l, &l.clone()).is_ok());
    // 内容が同じなら後継ではない。
    let mut same = l.clone();
    same.measurement_id = "m-2".into();
    assert!(check_successor(&l, &reseal(same.clone())).is_ok());
    assert!(check_successor(&l, &l.clone()).is_err());
}

#[test]
fn adding_a_nested_submodule_is_new_content_needing_a_new_measurement_id() {
    // 入れ子 submodule の snapshot を足すのは asset の変更。旧測定の lock には混ぜず、新しい測定 ID にだけ載る。
    let (_, l) = lock();
    let mut n = l.clone();
    let parent = n.cases[0].initial.as_ref().expect("i").submodules[0]
        .path
        .clone();
    n.cases[0]
        .initial
        .as_mut()
        .expect("i")
        .submodules
        .push(LockedSubmodule {
            path: format!("{parent}/inner/nested"),
            commit: g('7'),
            content_sha256: h('7'),
        });
    let same_id = reseal(n.clone());
    assert!(matches!(
        check_successor(&l, &same_id),
        Err(LockError::NotSuccessor(_))
    ));
    assert!(verify_lock(&l, &same_id).is_err());
    n.measurement_id = "m-2".into();
    let new = reseal(n);
    assert!(check_successor(&l, &new).is_ok());
    assert!(verify_lock(&l, &new).is_err());
    assert!(verify_lock(&l, &l.clone()).is_ok());
}
