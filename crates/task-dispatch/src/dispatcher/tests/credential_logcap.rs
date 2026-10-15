//! ADR 2026-10-15-mlsys-min-decision-assisted D4 / 受入「秘密の値を通常ログに出さない」: 待機 → secret 保存 → 検証
//! （成功・誤キー・権限不足・429）→ 再開の一連を tracing の捕捉 subscriber（全 level）の下で回し、
//! 捕捉した全ログに登録した値とその先頭・末尾 8 文字が含まれないことを断言する。外部ネットワーク・LLM は使わない。

use std::fmt::Write as _;
use std::sync::Mutex;

use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event as TracingEvent, Metadata, Subscriber};

use super::credential_gate::{SECRET, jev_pick, unit};
use super::*;
use crate::credential_resume::{
    CredentialResumer, CredentialVerifier, FileSecretSource, VerifyOutcome, secret_fingerprint,
};
use crate::dispatcher::{CredentialGate, CredentialGates};

/// 全 level の event と span の欄を 1 行ずつ文字列で溜める subscriber。
#[derive(Default)]
struct Capture {
    lines: Mutex<Vec<String>>,
}

struct Fields<'a>(&'a mut String);

impl Visit for Fields<'_> {
    fn record_debug(&mut self, f: &Field, v: &dyn std::fmt::Debug) {
        let _ = write!(self.0, " {}={v:?}", f.name());
    }
}

struct Sink(Arc<Capture>);

impl Subscriber for Sink {
    fn enabled(&self, _: &Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, a: &Attributes<'_>) -> Id {
        let mut s = format!("span {}", a.metadata().name());
        a.record(&mut Fields(&mut s));
        let mut lines = self.0.lines.lock().unwrap();
        lines.push(s);
        Id::from_u64(lines.len() as u64)
    }
    fn record(&self, _: &Id, v: &Record<'_>) {
        let mut s = String::from("span-record");
        v.record(&mut Fields(&mut s));
        self.0.lines.lock().unwrap().push(s);
    }
    fn record_follows_from(&self, _: &Id, _: &Id) {}
    fn event(&self, e: &TracingEvent<'_>) {
        let mut s = format!("{} {}:", e.metadata().level(), e.metadata().target());
        e.record(&mut Fields(&mut s));
        self.0.lines.lock().unwrap().push(s);
    }
    fn enter(&self, _: &Id) {}
    fn exit(&self, _: &Id) {}
}

struct Fixed(VerifyOutcome);

impl CredentialVerifier for Fixed {
    fn verify(&self, _secret_id: &str, _value: &str) -> VerifyOutcome {
        self.0
    }
}

/// 値・先頭 8 文字・末尾 8 文字（ログに出てはならない断片）。
fn fragments(value: &str) -> [String; 3] {
    let c: Vec<char> = value.chars().collect();
    [
        value.to_string(),
        c[..8].iter().collect(),
        c[c.len() - 8..].iter().collect(),
    ]
}

/// 待機 → 保存 → 検証（`outcome`）→（成功なら）再開を回し、捕捉した全ログを返す。
fn run_flow(outcome: VerifyOutcome, value: &str) -> Vec<String> {
    let dir = tempfile::tempdir().unwrap();
    let secrets = dir.path().join("secrets");
    std::fs::create_dir_all(&secrets).unwrap();
    let sqlite = Arc::new(SqliteStore::open(&dir.path().join("db.sqlite")).unwrap());
    let store: Arc<dyn TaskStore> = sqlite.clone();
    let now = OffsetDateTime::parse(
        "2026-10-15T00:00:00Z",
        &time::format_description::well_known::Rfc3339,
    )
    .unwrap();

    let cap = Arc::new(Capture::default());
    tracing::subscriber::with_default(Sink(cap.clone()), || {
        let mut d = pool_dispatcher(
            store.clone(),
            done_adapter(),
            None,
            PathBuf::from("/accounts/claude"),
            1,
            1,
        );
        d.set_credential_gates(CredentialGates {
            secrets_dir: Some(secrets.clone()),
            by_source: HashMap::from([(
                "jev".into(),
                CredentialGate {
                    secret_id: SECRET.into(),
                    provider: "jev".into(),
                    purpose: "Jev で依頼を分類する".into(),
                },
            )]),
        });
        let task = new_task(dir.path(), Check::Reviewer, 1);
        sqlite.insert(&task).unwrap();
        // callsite の interest は process 全体で cache される。並走する他の試験の thread が Sink の登録と
        // 同時に同じ callsite（待機入口の log）を初登録すると、古い「購読者なし（never）」が残り捕捉から
        // 漏れる。Sink 登録後の ここで作り直し、全 callsite が Sink を見た interest で決まるようにする。
        tracing::callsite::rebuild_interest_cache();
        let a = unit(&task, "a", 0);
        sqlite
            .work_units_apply(task.id, vec![a.clone()], Vec::new(), Vec::new())
            .unwrap();
        assert!(
            d.credential_gate_hold(&task, Some(&a), &jev_pick())
                .unwrap()
        );

        std::fs::write(secrets.join(SECRET), format!("{value}\n")).unwrap();
        sqlite
            .credential_record_secret_saved(SECRET, &secret_fingerprint(value), now)
            .unwrap();
        let r = CredentialResumer::new(
            sqlite.clone(),
            Arc::new(FileSecretSource {
                dir: secrets.clone(),
            }),
            Arc::new(move || now),
        )
        .with_verifier("jev", Arc::new(Fixed(outcome)));
        let rep = r.process().unwrap();
        // 報告・エラー表現にも値は出ない（ログと同じ面として捕捉に足す）。
        tracing::info!(report = ?rep, "resume pass finished");
    });
    cap.lines.lock().unwrap().clone()
}

#[test]
fn secret_values_never_appear_in_captured_logs_for_every_verify_outcome() {
    let cases = [
        (VerifyOutcome::Valid, "sk-logcap-valid-AAAA1111BBBB2222"),
        (
            VerifyOutcome::InvalidKey,
            "sk-logcap-wrong-CCCC3333DDDD4444",
        ),
        (
            VerifyOutcome::InsufficientPermission,
            "sk-logcap-noperm-EEEE5555FFFF6666",
        ),
        (VerifyOutcome::Transient, "sk-logcap-429-GGGG7777HHHH8888"),
    ];
    for (outcome, value) in cases {
        let lines = run_flow(outcome, value);
        // 空捕捉でない: 待機入口の log と、この試験が足した最終 log を実際に捕捉している。
        assert!(
            lines
                .iter()
                .any(|l| l.contains("credential missing; work unit waits")),
            "{outcome:?}: gate log not captured: {lines:?}"
        );
        assert!(lines.len() >= 2, "{outcome:?}: {lines:?}");
        let all = lines.join("\n");
        for frag in fragments(value) {
            assert!(
                !all.contains(&frag),
                "{outcome:?}: {frag} leaked into captured logs:\n{all}"
            );
        }
    }
}
