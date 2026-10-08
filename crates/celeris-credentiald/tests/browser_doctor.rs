//! The read-only Ping still requires the caller's admitted PID, never changes vault state.
use std::os::unix::fs::PermissionsExt;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

struct Guard(Child);
impl Drop for Guard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn browser_doctor_ping_requires_control_pid_admission() {
    for admitted in [true, false] {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let runtime = root.path().join("run");
        for dir in [
            home.join(".config/celeris"),
            home.join(".local/celeris"),
            runtime.clone(),
        ] {
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let child =
            Command::new(std::env::var("CARGO_BIN_EXE_celeris-credentiald").expect("binary path"))
                .args([
                    "serve",
                    &if admitted {
                        std::process::id().to_string()
                    } else {
                        "1".into()
                    },
                ])
                .env_clear()
                .env("HOME", &home)
                .env("XDG_RUNTIME_DIR", &runtime)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap();
        let mut child = Guard(child);
        let socket = runtime.join("celeris-credentiald/control.sock");
        let deadline = Instant::now() + Duration::from_secs(60);
        while !socket.exists() {
            assert!(
                child.0.try_wait().unwrap().is_none(),
                "broker exited before readiness"
            );
            assert!(Instant::now() < deadline, "broker readiness timeout");
            std::thread::sleep(Duration::from_millis(10));
        }
        let reply = celeris_credentiald::ipc::call(&socket, br#"{"op":"ping"}"#).unwrap();
        assert_eq!(reply.success, admitted);
        if !admitted {
            assert_eq!(reply.code.as_deref(), Some("permission_denied"));
        }
        assert!(reply.credential.is_none());
        assert!(reply.binding_token.is_none());
        assert!(reply.lease_id.is_none());
    }
}
