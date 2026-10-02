//! ADR-0095: worker の run から本番 DB は読み取り専用（D1〜D5）。
//!
//! celeris が task のために起動するプロセス（adapter・check。コンテナ実行を除く）は、exec の直前（fork 後の
//! 子、`pre_exec`）に unprivileged な user + mount namespace に入り、**DB のディレクトリを読み取り専用**に
//! する。DB 本体と `-wal` / `-shm` / `-journal` 以外の直下の項目は自分自身へ bind し直すので従来どおり書ける。
//! プロセスの継承で効くので、エージェント CLI の sandbox の有無・escalation・子プロセスに関係なく掛かる。
//!
//! - 入口は [`launch`]（`container::wrap` の代わり）。コンテナ実行ならそのまま `container::wrap`、そうでなければ
//!   [`install`] されたガードを [`apply`] する（無ければ何もしない）。
//! - 準備（直下の列挙・`statvfs`・uid/gid・cwd の絶対化・ssh 設定の写し）は fork の**前**に親で行い、子では
//!   用意した C 文字列で `unshare` / `mount` / `chdir` を呼ぶだけ（割り当てをしない）。
//! - 付記 D-a: user systemd bus を tmpfs / 空ファイルの bind で覆い、`$HOME` の `.config/systemd`・
//!   `.local/celeris/releases`・`.config/celeris` を（存在すれば）読み取り専用にする（[`DbGuard::host_config_read_only_paths`]）。
//! - 準備に失敗したら、その spawn を失敗させる（黙って保護なしで起動しない）。

use std::ffi::{CString, OsString};
use std::io;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use nix::libc;

/// ssh の `Include` 先（D4）。root 所有のファイルが namespace の中では nobody に見え、ssh が拒否する。
const SSH_CONFIG_D: &str = "/etc/ssh/ssh_config.d";

/// 付記 D-a の 3: namespace の中で読み取り専用にする `$HOME` 下の host の設定（存在するものだけ）。
const HOST_CONFIG_DIRS: [&str; 3] = [
    ".config/systemd",
    ".local/celeris/releases",
    ".config/celeris",
];

#[derive(Debug, thiserror::Error)]
pub enum DbGuardError {
    #[error("db {path}: {source}")]
    Db { path: PathBuf, source: io::Error },
    #[error("db {0} has no parent directory")]
    NoParent(PathBuf),
    #[error("probe failed: {0}")]
    Probe(String),
}

/// 保護する DB（`[db] path`）。`DbGuard::new` は DB ファイルが存在することを要求する（daemon が開いた後に作る）。
#[derive(Debug, Clone)]
pub struct DbGuard {
    db: PathBuf,
    dir: PathBuf,
    /// 読み取り専用のまま残す直下の名前（DB 本体と `-wal` / `-shm` / `-journal`）。
    family: Vec<OsString>,
    /// ssh 設定の写しを置く所（D4）。`None` なら写さない。
    ssh_shadow_root: Option<PathBuf>,
    /// 付記 D-a の 3 の基準の `$HOME`（`new` で解決。`None` なら何もしない）。
    home: Option<PathBuf>,
}

impl DbGuard {
    pub fn new(db_path: &Path) -> Result<Self, DbGuardError> {
        let db = std::fs::canonicalize(db_path).map_err(|source| DbGuardError::Db {
            path: db_path.to_path_buf(),
            source,
        })?;
        let dir = db
            .parent()
            .ok_or_else(|| DbGuardError::NoParent(db.clone()))?
            .to_path_buf();
        let name = db
            .file_name()
            .ok_or_else(|| DbGuardError::NoParent(db.clone()))?
            .to_os_string();
        let family = ["", "-wal", "-shm", "-journal"]
            .iter()
            .map(|suffix| {
                let mut n = name.clone();
                n.push(suffix);
                n
            })
            .collect();
        Ok(Self {
            db,
            dir,
            family,
            ssh_shadow_root: Some(default_ssh_shadow_root()),
            home: std::env::var_os("HOME")
                .filter(|h| !h.is_empty())
                .map(PathBuf::from)
                .filter(|h| h.is_absolute()),
        })
    }

    /// 試験用: 付記 D-a の 3 の基準の `$HOME` を変える（`None` で読み取り専用にしない）。
    pub fn with_home(mut self, home: Option<PathBuf>) -> Self {
        self.home = home;
        self
    }

    /// 付記 D-a の 3: `$HOME` 下の host の設定のうち、存在するもの（canonicalize 済み）。存在するかを
    /// 確かめられない（`EACCES` など）ときは失敗にする（D5: 保護なしで起動しない）。
    pub fn host_config_read_only_paths(&self) -> io::Result<Vec<PathBuf>> {
        let Some(home) = &self.home else {
            return Ok(Vec::new());
        };
        let mut out = Vec::new();
        for rel in HOST_CONFIG_DIRS {
            let path = home.join(rel);
            if path.try_exists()? {
                out.push(std::fs::canonicalize(&path)?);
            }
        }
        Ok(out)
    }

    /// 試験用: ssh 設定の写しの置き場所を変える（`None` で写さない）。
    pub fn with_ssh_shadow_root(mut self, root: Option<PathBuf>) -> Self {
        self.ssh_shadow_root = root;
        self
    }

    /// canonicalize 済みの DB のパス。
    pub fn db_path(&self) -> &Path {
        &self.db
    }

    /// 読み取り専用にするディレクトリ（DB の親）。
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// 直下のうち、書けるように bind し直す項目（ディレクトリと通常ファイル。symlink は辿らない）。
    /// spawn のたびに列挙する（daemon の起動後に増えた兄弟も書ける）。
    pub fn writable_children(&self) -> io::Result<Vec<PathBuf>> {
        let mut out = Vec::new();
        for entry in std::fs::read_dir(&self.dir)? {
            let entry = entry?;
            let name = entry.file_name();
            if self.family.contains(&name) {
                continue;
            }
            let ty = entry.file_type()?;
            if ty.is_dir() || ty.is_file() {
                out.push(self.dir.join(name));
            }
        }
        out.sort();
        Ok(out)
    }

    fn plan(&self, cwd: Option<&Path>, runtime_dir: Option<PathBuf>) -> io::Result<Plan> {
        let keep = self
            .writable_children()?
            .into_iter()
            .map(|p| cstring(p.into_os_string()))
            .collect::<io::Result<Vec<_>>>()?;
        let stat = nix::sys::statvfs::statvfs(&self.dir).map_err(io::Error::from)?;
        let cwd = match cwd {
            Some(c) if c.is_absolute() => Some(c.to_path_buf()),
            Some(c) => std::env::current_dir().ok().map(|d| d.join(c)),
            None => std::env::current_dir().ok(),
        };
        let ssh = match &self.ssh_shadow_root {
            Some(root) => sync_ssh_shadow(Path::new(SSH_CONFIG_D), root)
                .ok()
                .flatten()
                .map(|shadow| -> io::Result<(CString, CString)> {
                    Ok((
                        cstring(shadow.into_os_string())?,
                        cstring(OsString::from(SSH_CONFIG_D))?,
                    ))
                })
                .transpose()?,
            None => None,
        };
        let read_only = self
            .host_config_read_only_paths()?
            .into_iter()
            .map(|p| -> io::Result<(CString, libc::c_ulong)> {
                let stat = nix::sys::statvfs::statvfs(&p).map_err(io::Error::from)?;
                Ok((
                    cstring(p.into_os_string())?,
                    libc::MS_REMOUNT
                        | libc::MS_BIND
                        | libc::MS_RDONLY
                        | locked_mount_flags(stat.flags()),
                ))
            })
            .collect::<io::Result<Vec<_>>>()?;
        let uid = nix::unistd::getuid().as_raw();
        let gid = nix::unistd::getgid().as_raw();
        let mut bus_paths = Vec::new();
        let host_bus = PathBuf::from(format!("/run/user/{uid}/bus"));
        if host_bus.exists() {
            bus_paths.push(cstring(host_bus.into_os_string())?);
        }
        let mut systemd_dir = None;
        if let Some(runtime) = runtime_dir.filter(|p| !p.as_os_str().is_empty()) {
            if !runtime.is_absolute() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "XDG_RUNTIME_DIR is not absolute",
                ));
            }
            let bus = runtime.join("bus");
            if bus.exists()
                && !bus_paths
                    .iter()
                    .any(|p| p.as_bytes() == bus.as_os_str().as_bytes())
            {
                bus_paths.push(cstring(bus.into_os_string())?);
            }
            let systemd = runtime.join("systemd");
            if systemd.exists() {
                systemd_dir = Some(cstring(systemd.into_os_string())?);
            }
        }
        // Bind mounts from anonymous fds (memfd/O_TMPFILE) fail on socket targets.
        // NamedTempFile keeps a randomly named, empty regular file alive through pre_exec.
        let empty_bus = if bus_paths.is_empty() {
            None
        } else {
            Some(tempfile::NamedTempFile::new()?)
        };
        let empty_bus_path = empty_bus
            .as_ref()
            .map(|file| cstring(file.path().as_os_str().to_os_string()))
            .transpose()?;
        Ok(Plan {
            dir: cstring(self.dir.clone().into_os_string())?,
            keep,
            remount_flags: libc::MS_REMOUNT
                | libc::MS_BIND
                | libc::MS_RDONLY
                | locked_mount_flags(stat.flags()),
            cwd: cwd.map(|c| cstring(c.into_os_string())).transpose()?,
            uid_map: format!("{uid} {uid} 1").into_bytes(),
            gid_map: format!("{gid} {gid} 1").into_bytes(),
            ssh,
            read_only,
            bus_paths,
            systemd_dir,
            _empty_bus: empty_bus,
            empty_bus_path,
        })
    }
}

/// fork の前に作る、子で使う値（C 文字列と数値だけ）。
struct Plan {
    dir: CString,
    keep: Vec<CString>,
    remount_flags: libc::c_ulong,
    cwd: Option<CString>,
    uid_map: Vec<u8>,
    gid_map: Vec<u8>,
    /// (写し, `/etc/ssh/ssh_config.d`)
    ssh: Option<(CString, CString)>,
    /// 付記 D-a の 3: 自分自身へ bind して読み取り専用にする path と、その remount の flag。
    read_only: Vec<(CString, libc::c_ulong)>,
    bus_paths: Vec<CString>,
    systemd_dir: Option<CString>,
    /// Retains the source file until all bus socket paths have been overmounted.
    _empty_bus: Option<tempfile::NamedTempFile>,
    empty_bus_path: Option<CString>,
}

impl Plan {
    /// 子（fork 後・exec 前）で呼ぶ。割り当てをせず、libc の呼び出しだけを行う。
    fn enter(&self) -> io::Result<()> {
        // SAFETY: fork 後の子で、親が用意した NUL 終端の文字列だけを渡す。どれも async-signal-safe な
        // システムコールの薄い包み。
        unsafe {
            check(libc::unshare(libc::CLONE_NEWUSER | libc::CLONE_NEWNS))?;
            write_proc(c"/proc/self/setgroups", b"deny")?;
            write_proc(c"/proc/self/uid_map", &self.uid_map)?;
            write_proc(c"/proc/self/gid_map", &self.gid_map)?;
            check(libc::mount(
                std::ptr::null(),
                c"/".as_ptr(),
                std::ptr::null(),
                libc::MS_REC | libc::MS_PRIVATE,
                std::ptr::null(),
            ))?;
            for child in &self.keep {
                bind(child, child, libc::MS_REC)?;
            }
            if let Some((shadow, target)) = &self.ssh {
                // D4 は best-effort（失敗しても DB の保護には関係しない）。
                let _ = bind(shadow, target, 0);
            }
            if let Some(systemd) = &self.systemd_dir {
                check(libc::mount(
                    c"tmpfs".as_ptr(),
                    systemd.as_ptr(),
                    c"tmpfs".as_ptr(),
                    libc::MS_NOSUID | libc::MS_NODEV | libc::MS_NOEXEC,
                    c"size=4096,mode=0700".as_ptr().cast(),
                ))?;
            }
            if let Some(source) = &self.empty_bus_path {
                for target in &self.bus_paths {
                    bind(source, target, 0)?;
                }
            }
            for (path, flags) in &self.read_only {
                bind(path, path, 0)?;
                check(libc::mount(
                    std::ptr::null(),
                    path.as_ptr(),
                    std::ptr::null(),
                    *flags,
                    std::ptr::null(),
                ))?;
            }
            bind(&self.dir, &self.dir, libc::MS_REC)?;
            check(libc::mount(
                std::ptr::null(),
                self.dir.as_ptr(),
                std::ptr::null(),
                self.remount_flags,
                std::ptr::null(),
            ))?;
            // std は pre_exec より前に cwd へ chdir するので、cwd は mount の下の古い dentry を指している。
            // 絶対パスで入り直し、`../celeris.sqlite3` が元の（書ける）mount に届かないようにする。
            if let Some(cwd) = &self.cwd {
                check(libc::chdir(cwd.as_ptr()))?;
            }
        }
        Ok(())
    }
}

unsafe fn bind(src: &CString, dst: &CString, extra: libc::c_ulong) -> io::Result<()> {
    // SAFETY: 呼び出し側（`Plan::enter`）と同じ。
    check(unsafe {
        libc::mount(
            src.as_ptr(),
            dst.as_ptr(),
            std::ptr::null(),
            libc::MS_BIND | extra,
            std::ptr::null(),
        )
    })
}

unsafe fn write_proc(path: &std::ffi::CStr, data: &[u8]) -> io::Result<()> {
    // SAFETY: 呼び出し側（`Plan::enter`）と同じ。
    unsafe {
        let fd = libc::open(path.as_ptr(), libc::O_WRONLY | libc::O_CLOEXEC);
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let n = libc::write(fd, data.as_ptr().cast(), data.len());
        let err = io::Error::last_os_error();
        libc::close(fd);
        if n < 0 || n as usize != data.len() {
            return Err(err);
        }
    }
    Ok(())
}

fn check(rc: libc::c_int) -> io::Result<()> {
    if rc == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

fn cstring(s: OsString) -> io::Result<CString> {
    CString::new(s.into_vec()).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))
}

fn runtime_dir(command: &std::process::Command) -> Option<PathBuf> {
    command
        .get_envs()
        .find(|(name, _)| *name == "XDG_RUNTIME_DIR")
        .map(|(_, value)| value.map(PathBuf::from))
        .unwrap_or_else(|| std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from))
}

/// user namespace では元の mount の `nosuid` / `nodev` / `noexec` / atime 系は外せない（locked）ので、
/// 読み取り専用にする remount でも引き継ぐ。
fn locked_mount_flags(flags: nix::sys::statvfs::FsFlags) -> libc::c_ulong {
    use nix::sys::statvfs::FsFlags;
    let mut out = 0;
    for (st, ms) in [
        (FsFlags::ST_NOSUID, libc::MS_NOSUID),
        (FsFlags::ST_NODEV, libc::MS_NODEV),
        (FsFlags::ST_NOEXEC, libc::MS_NOEXEC),
        (FsFlags::ST_NOATIME, libc::MS_NOATIME),
        (FsFlags::ST_NODIRATIME, libc::MS_NODIRATIME),
        (FsFlags::ST_RELATIME, libc::MS_RELATIME),
    ] {
        if flags.contains(st) {
            out |= ms;
        }
    }
    out
}

fn default_ssh_shadow_root() -> PathBuf {
    match std::env::var_os("XDG_RUNTIME_DIR").filter(|v| !v.is_empty()) {
        Some(dir) => PathBuf::from(dir).join("celeris-db-guard"),
        None => std::env::temp_dir().join(format!(
            "celeris-db-guard-{}",
            nix::unistd::getuid().as_raw()
        )),
    }
}

/// D4: `src`（`/etc/ssh/ssh_config.d`）の通常ファイル（symlink は辿って中身を読む）を `root/ssh_config.d` へ
/// 同じ内容で写す（この uid の所有になる）。`src` が無ければ `Ok(None)`。内容が同じファイルは書かない。
/// 写しにだけある名前は消す。書き込みは一時ファイル + rename（並行する spawn が半端な中身を読まない）。
pub fn sync_ssh_shadow(src: &Path, root: &Path) -> io::Result<Option<PathBuf>> {
    if !src.is_dir() {
        return Ok(None);
    }
    let shadow = root.join("ssh_config.d");
    std::fs::create_dir_all(&shadow)?;
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o700));
    }
    let mut names: Vec<OsString> = Vec::new();
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let name = entry.file_name();
        let Ok(content) = std::fs::read(&path) else {
            continue;
        };
        let dest = shadow.join(&name);
        if std::fs::read(&dest).ok().as_deref() != Some(content.as_slice()) {
            let mut tmp_name = OsString::from(".tmp-");
            tmp_name.push(format!(
                "{}-{}-",
                std::process::id(),
                TMP_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            tmp_name.push(&name);
            let tmp = shadow.join(tmp_name);
            std::fs::write(&tmp, &content)?;
            std::fs::rename(&tmp, &dest)?;
        }
        names.push(name);
    }
    for entry in std::fs::read_dir(&shadow)? {
        let entry = entry?;
        let name = entry.file_name();
        if !names.contains(&name) && !name.as_bytes().starts_with(b".tmp-") {
            let _ = std::fs::remove_file(entry.path());
        }
    }
    Ok(Some(shadow))
}

static TMP_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// `command` に D1 の `pre_exec` を付ける。準備に失敗したら、その理由を log に残し、spawn が同じ errno で
/// 失敗するようにする（保護なしで起動しない）。
pub fn apply(command: &mut tokio::process::Command, guard: &DbGuard) {
    // 守る DB がもう無い（同じプロセスで先に終わった daemon〈in-process の試験〉が入れたガードの残り）なら
    // 守るものが無いので掛けない。本番の daemon の DB は動いている間に消えない（namespace の中からは消せない）。
    if !guard.db.exists() {
        tracing::warn!(
            db = %guard.db.display(),
            "worker db guard (ADR-0095): the guarded db no longer exists; spawning without the namespace"
        );
        return;
    }
    // ADR-0126 A1-1: worker run の印（守る DB のディレクトリの path だけ）。
    command.env(WORKER_DB_GUARD_ENV, &guard.dir);
    let cwd = command.as_std().get_current_dir().map(Path::to_path_buf);
    match guard.plan(cwd.as_deref(), runtime_dir(command.as_std())) {
        Ok(plan) => {
            // SAFETY: `Plan::enter` は fork 後の子で libc の呼び出しだけを行い、割り当てをしない。
            unsafe {
                command.pre_exec(move || plan.enter());
            }
        }
        Err(e) => {
            tracing::error!(
                db = %guard.db.display(),
                "worker db guard (ADR-0095): cannot prepare the read-only namespace, refusing to spawn: {e}"
            );
            let errno = e.raw_os_error().unwrap_or(libc::EPERM);
            // SAFETY: 何も呼ばずに errno を返すだけ。
            unsafe {
                command.pre_exec(move || Err(io::Error::from_raw_os_error(errno)));
            }
        }
    }
}

/// `apply` の std 版（起動時の probe と試験用）。
pub fn apply_std(command: &mut std::process::Command, guard: &DbGuard) -> io::Result<()> {
    use std::os::unix::process::CommandExt;
    let plan = guard.plan(command.get_current_dir(), runtime_dir(command))?;
    // ADR-0126 A1-1: [`apply`] と同じ印。
    command.env(WORKER_DB_GUARD_ENV, &guard.dir);
    // SAFETY: `apply` と同じ。
    unsafe {
        command.pre_exec(move || plan.enter());
    }
    Ok(())
}

/// D5/D-a: 起動時に namespace 付きプロセスで DB の読み取り専用化と user bus の遮断を確かめる。
pub fn probe(guard: &DbGuard) -> Result<(), DbGuardError> {
    let mut command = std::process::Command::new("sh");
    command
        .arg("-c")
        .arg("test ! -w \"$1\" && test ! -S \"/run/user/$(id -u)/bus\" && { test -z \"$XDG_RUNTIME_DIR\" || { test ! -S \"$XDG_RUNTIME_DIR/bus\" && test ! -e \"$XDG_RUNTIME_DIR/systemd/private\"; }; }")
        .arg("sh")
        .arg(guard.db_path())
        .env_remove("DBUS_SESSION_BUS_ADDRESS")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped());
    apply_std(&mut command, guard).map_err(|e| DbGuardError::Probe(e.to_string()))?;
    let out = command
        .output()
        .map_err(|e| DbGuardError::Probe(format!("spawn in the namespace failed: {e}")))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(DbGuardError::Probe(format!(
            "{} is writable or the user systemd bus is visible inside the namespace ({}; {})",
            guard.db_path().display(),
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        )))
    }
}

static INSTALLED: RwLock<Option<Arc<DbGuard>>> = RwLock::new(None);

/// D5: daemon の `run` が入れるプロセス全体のガード（`None` で外す）。後から入れたものが勝つ。
pub fn install(guard: Option<DbGuard>) {
    let guard = guard.map(Arc::new);
    match INSTALLED.write() {
        Ok(mut slot) => *slot = guard,
        Err(poisoned) => *poisoned.into_inner() = guard,
    }
}

/// いま入っているガード。
pub fn installed() -> Option<Arc<DbGuard>> {
    match INSTALLED.read() {
        Ok(slot) => slot.clone(),
        Err(poisoned) => poisoned.into_inner().clone(),
    }
}

/// D2: worker の run のプロセスを spawn する所で `container::wrap` の代わりに呼ぶ。コンテナ実行なら
/// `container::wrap`、そうでなければ入っているガードを付ける（無ければそのまま）。
pub fn launch(
    command: tokio::process::Command,
    container: Option<&crate::container::ContainerPlan>,
) -> tokio::process::Command {
    launch_with(command, container, installed().as_deref())
}

/// [`launch`] の本体。ガードを引数で受ける（試験はプロセス全体の [`install`] を使わずにここを呼ぶ。
/// install すると同じ test binary で並行する他の試験の spawn にも試験用の DB のガードが掛かり、その tempdir が
/// 消えた瞬間に `pre_exec` の bind が ENOENT になる）。
fn launch_with(
    mut command: tokio::process::Command,
    container: Option<&crate::container::ContainerPlan>,
    guard: Option<&DbGuard>,
) -> tokio::process::Command {
    command.env_remove("DBUS_SESSION_BUS_ADDRESS");
    if container.is_some() {
        return crate::container::wrap(command, container);
    }
    if let Some(guard) = guard {
        apply(&mut command, guard);
    }
    command
}

// ---------------------------------------------------------------------------
// ADR-0126 A: 守る対象（本番 DB・本番 token）と worker run の印で、試験 daemon に userns を求めるかを決める。
// ---------------------------------------------------------------------------

/// ADR-0126 A1-1: worker run の印。本番 daemon の [`apply`] / [`apply_std`] が子の環境に
/// `<正規化した守る DB のディレクトリ>` を入れる（path だけ。token の値は入れない）。
pub const WORKER_DB_GUARD_ENV: &str = "CELERIS_WORKER_DB_GUARD";

/// ADR-0126 A2: worker run の中で本番を使う daemon を拒否するときに足す文言。
pub const PRODUCTION_IN_WORKER_RUN: &str =
    "worker db guard: this daemon uses the production DB/token inside a worker run (ADR-0126)";

/// canonicalize した path と、存在すれば `(st_dev, st_ino)`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedPath {
    pub path: PathBuf,
    /// 存在しない path（末尾が未作成）なら `None`。
    pub id: Option<(u64, u64)>,
}

impl ResolvedPath {
    /// 存在する path だけを受ける（symlink・相対 path・`..` を解き、inode を控える）。
    pub fn resolve(path: &Path) -> io::Result<Self> {
        use std::os::unix::fs::MetadataExt;
        let canonical = std::fs::canonicalize(path)?;
        let meta = std::fs::metadata(&canonical)?;
        Ok(Self {
            path: canonical,
            id: Some((meta.dev(), meta.ino())),
        })
    }

    /// 末尾がまだ無い path も受ける（存在する最も近い祖先を canonicalize し、残りの名前を足す）。
    /// 宙吊りの symlink・末尾が `..` の path は解けないので失敗にする。
    pub fn resolve_lenient(path: &Path) -> io::Result<Self> {
        match Self::resolve(path) {
            Ok(r) => Ok(r),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                if std::fs::symlink_metadata(path).is_ok() {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!("{} is a dangling symlink", path.display()),
                    ));
                }
                let name = path.file_name().ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!("{} has no file name", path.display()),
                    )
                })?;
                let parent = match path.parent() {
                    Some(p) if !p.as_os_str().is_empty() => Self::resolve_lenient(p)?.path,
                    Some(_) => std::env::current_dir()?,
                    None => {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidInput,
                            format!("{} has no parent", path.display()),
                        ));
                    }
                };
                Ok(Self {
                    path: parent.join(name),
                    id: None,
                })
            }
            Err(e) => Err(e),
        }
    }

    fn same(&self, other: &ResolvedPath) -> bool {
        self.path == other.path || (self.id.is_some() && self.id == other.id)
    }

    /// 同じか、どちらかがもう一方の中（component 単位の前方一致）か、同じ inode。
    fn overlaps(&self, other: &ResolvedPath) -> bool {
        self.same(other) || self.path.starts_with(&other.path) || other.path.starts_with(&self.path)
    }
}

/// ADR-0126 A1: 本番の守る対象の集合 P。決められない部分があれば `unknown` に理由を積み、免除しない（fail closed）。
/// token の内容は保持するが `Debug` には出さない。
#[derive(Default)]
pub struct ProtectedSet {
    db_files: Vec<ResolvedPath>,
    /// 本番 DB のディレクトリ・`state_dir`・印の path。
    dirs: Vec<ResolvedPath>,
    token_files: Vec<ResolvedPath>,
    tokens: Vec<zeroize::Zeroizing<Vec<u8>>>,
    /// 本番 token が P にあるのに内容を読めなかった。
    token_unreadable: bool,
    unknown: Vec<String>,
}

impl std::fmt::Debug for ProtectedSet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProtectedSet")
            .field("db_files", &self.db_files)
            .field("dirs", &self.dirs)
            .field("token_files", &self.token_files)
            .field("tokens", &format_args!("<{} redacted>", self.tokens.len()))
            .field("token_unreadable", &self.token_unreadable)
            .field("unknown", &self.unknown)
            .finish()
    }
}

impl ProtectedSet {
    pub fn new() -> Self {
        Self::default()
    }

    /// 本番 config の `[db] path`。DB ファイルとそのディレクトリを P に入れる。
    pub fn add_db_file(&mut self, db: &Path) {
        match ResolvedPath::resolve_lenient(db) {
            Ok(r) => {
                if let Some(parent) = r.path.parent() {
                    self.add_dir(parent);
                }
                self.db_files.push(r);
            }
            Err(e) => self.mark_unknown(format!("production db {}: {e}", db.display())),
        }
    }

    /// 本番 config の `state_dir`（と印の path）。
    pub fn add_dir(&mut self, dir: &Path) {
        match ResolvedPath::resolve_lenient(dir) {
            Ok(r) => {
                if !self.dirs.contains(&r) {
                    self.dirs.push(r);
                }
            }
            Err(e) => self.mark_unknown(format!("production dir {}: {e}", dir.display())),
        }
    }

    /// 本番 config の `[api] token_file`。path と内容を P に入れる（無ければ path だけ）。
    pub fn add_token_file(&mut self, token_file: &Path) {
        match ResolvedPath::resolve_lenient(token_file) {
            Ok(r) => self.token_files.push(r),
            Err(e) => {
                self.mark_unknown(format!(
                    "production token file {}: {e}",
                    token_file.display()
                ));
                return;
            }
        }
        match std::fs::read(token_file) {
            Ok(content) => self.tokens.push(zeroize::Zeroizing::new(content)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(_) => self.token_unreadable = true,
        }
    }

    /// P を決められない（本番 config が読めない・parse できない、など）。
    pub fn mark_unknown(&mut self, reason: impl Into<String>) {
        self.unknown.push(reason.into());
    }

    /// 決められなかった理由（空なら P は決まっている）。
    pub fn unknown_reasons(&self) -> &[String] {
        &self.unknown
    }

    fn token_matches(&self, content: &[u8]) -> bool {
        // 全部と比べる（早く抜けない）。
        self.tokens
            .iter()
            .fold(false, |hit, t| constant_time_eq(t, content) | hit)
    }
}

/// token の比較。前後の空白（末尾の改行）を除き、長さが違っても全バイトを見る。
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    let a = a.trim_ascii();
    let b = b.trim_ascii();
    let n = a.len().max(b.len());
    let mut diff = (a.len() ^ b.len()) as u64;
    for i in 0..n {
        let x = a.get(i).copied().unwrap_or(0);
        let y = b.get(i).copied().unwrap_or(0);
        diff |= u64::from(x ^ y);
    }
    std::hint::black_box(diff) == 0
}

/// 起動しようとしている daemon の path（その config から。相対 path・symlink のままでよい）。
#[derive(Debug, Clone)]
pub struct DaemonPaths {
    pub db: PathBuf,
    pub state_dir: Option<PathBuf>,
    pub token_file: Option<PathBuf>,
}

/// ADR-0126 A3-2: worker run の印（`CELERIS_WORKER_DB_GUARD`）の状態。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkerRunMarker {
    /// 印が無い（worker run の外）。
    Absent,
    /// 印はあるが、その path が読み取り専用であることを kernel で裏付けられない（偽の印・guard の外）。
    Unverified { value: PathBuf, reason: String },
    /// 印の path（canonicalize 済み）が今の mount namespace で読み取り専用。
    ReadOnly(PathBuf),
}

impl WorkerRunMarker {
    fn present(&self) -> bool {
        !matches!(self, WorkerRunMarker::Absent)
    }
}

/// ADR-0126 A3-2: 環境の `CELERIS_WORKER_DB_GUARD` を読み、[`verify_worker_run_marker`] で裏付ける。
pub fn detect_worker_run_marker() -> WorkerRunMarker {
    verify_worker_run_marker(std::env::var_os(WORKER_DB_GUARD_ENV))
}

/// 印の値が、絶対 path で、`statvfs` が `ST_RDONLY` を返し、かつそこへの書き込み試行が `EROFS` で
/// 失敗するときだけ [`WorkerRunMarker::ReadOnly`]。どれかを確かめられなければ `Unverified`。
pub fn verify_worker_run_marker(value: Option<OsString>) -> WorkerRunMarker {
    let Some(value) = value.filter(|v| !v.is_empty()) else {
        return WorkerRunMarker::Absent;
    };
    let value = PathBuf::from(value);
    let unverified = |reason: String| WorkerRunMarker::Unverified {
        value: value.clone(),
        reason,
    };
    if !value.is_absolute() {
        return unverified("not an absolute path".into());
    }
    let canonical = match std::fs::canonicalize(&value) {
        Ok(p) => p,
        Err(e) => return unverified(format!("canonicalize: {e}")),
    };
    match nix::sys::statvfs::statvfs(&canonical) {
        Ok(st) if st.flags().contains(nix::sys::statvfs::FsFlags::ST_RDONLY) => {}
        Ok(_) => return unverified("statvfs does not report ST_RDONLY".into()),
        Err(e) => return unverified(format!("statvfs: {e}")),
    }
    let probe = canonical.join(format!(
        ".celeris-db-guard-probe-{}-{}",
        std::process::id(),
        TMP_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
    {
        Err(e) if e.raw_os_error() == Some(libc::EROFS) => WorkerRunMarker::ReadOnly(canonical),
        Err(e) => unverified(format!("write attempt did not fail with EROFS: {e}")),
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            unverified("write attempt succeeded".into())
        }
    }
}

/// ADR-0126 A2/A3 の判定結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuardDecision {
    /// A3: 試験用の一時 DB を guard の効いた worker run の中で使う。probe をせず guard を入れない。
    Exempt,
    /// A2: 本番を使う daemon。従来どおり probe 必須（worker run の中なら [`PRODUCTION_IN_WORKER_RUN`] で失敗）。
    RefuseProduction {
        reason: String,
        inside_worker_run: bool,
    },
    /// 免除の条件を満たさない（worker run の外・印が裏付けられない・P を決められない）。従来どおり probe。
    RequireUserns { reason: String },
}

impl GuardDecision {
    /// probe が失敗したときに足す文言（本番を worker run の中で使うときだけ）。
    pub fn refusal_message(&self) -> Option<&'static str> {
        match self {
            GuardDecision::RefuseProduction {
                inside_worker_run: true,
                ..
            } => Some(PRODUCTION_IN_WORKER_RUN),
            _ => None,
        }
    }
}

/// ADR-0126 A2/A3: userns・実 mount に依らない判定。拒否（A2）を先に見て、どれか 1 つに当たれば
/// `RefuseProduction`。拒否に当たらず、印が読み取り専用で裏付けられ、P が決まっているときだけ `Exempt`。
/// 判定できないときは免除しない（fail closed）。token の値はどこにも出さない。
pub fn judge_worker_db_guard(
    protected: &ProtectedSet,
    daemon: &DaemonPaths,
    marker: &WorkerRunMarker,
) -> GuardDecision {
    let inside_worker_run = marker.present();
    let refuse = |reason: String| GuardDecision::RefuseProduction {
        reason,
        inside_worker_run,
    };
    // A3-3: 印の path も P に入れて A2 を当てる。
    let mut dirs = protected.dirs.clone();
    let mut unknown = protected.unknown.clone();
    if let WorkerRunMarker::ReadOnly(path) = marker {
        match ResolvedPath::resolve(path) {
            Ok(r) => dirs.push(r),
            Err(e) => unknown.push(format!("marker {}: {e}", path.display())),
        }
    }

    let db = match ResolvedPath::resolve(&daemon.db) {
        Ok(r) => r,
        Err(e) => {
            return refuse(format!(
                "cannot resolve the daemon db {}: {e}",
                daemon.db.display()
            ));
        }
    };
    if let Some(p) = protected.db_files.iter().find(|p| db.same(p)) {
        return refuse(format!(
            "db {} is the production db {}",
            db.path.display(),
            p.path.display()
        ));
    }
    let mut daemon_dirs = Vec::new();
    match db.path.parent().map(ResolvedPath::resolve) {
        Some(Ok(r)) => daemon_dirs.push(r),
        Some(Err(e)) => return refuse(format!("cannot resolve the daemon db directory: {e}")),
        None => return refuse(format!("db {} has no parent directory", db.path.display())),
    }
    if let Some(state_dir) = &daemon.state_dir {
        match ResolvedPath::resolve_lenient(state_dir) {
            Ok(r) => daemon_dirs.push(r),
            Err(e) => {
                return refuse(format!(
                    "cannot resolve the daemon state_dir {}: {e}",
                    state_dir.display()
                ));
            }
        }
    }
    for d in &daemon_dirs {
        if let Some(p) = dirs.iter().find(|p| d.overlaps(p)) {
            return refuse(format!(
                "{} overlaps the production directory {}",
                d.path.display(),
                p.path.display()
            ));
        }
    }
    if let Some(token_file) = &daemon.token_file {
        let t = match ResolvedPath::resolve_lenient(token_file) {
            Ok(r) => r,
            Err(e) => {
                return refuse(format!(
                    "cannot resolve the daemon token_file {}: {e}",
                    token_file.display()
                ));
            }
        };
        if protected.token_files.iter().any(|p| t.same(p)) {
            return refuse(format!(
                "token_file {} is the production token file",
                t.path.display()
            ));
        }
        if protected.token_unreadable {
            return refuse(
                "the production token is unreadable and the daemon has a token_file".into(),
            );
        }
        match std::fs::read(&t.path) {
            Ok(content) => {
                let content = zeroize::Zeroizing::new(content);
                if protected.token_matches(&content) {
                    return refuse(format!(
                        "token_file {} holds the production token",
                        t.path.display()
                    ));
                }
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => {
                return refuse(format!(
                    "cannot read the daemon token_file {}: {e}",
                    t.path.display()
                ));
            }
        }
    }

    match marker {
        WorkerRunMarker::Absent => GuardDecision::RequireUserns {
            reason: format!("{WORKER_DB_GUARD_ENV} is not set (outside a worker run)"),
        },
        WorkerRunMarker::Unverified { value, reason } => GuardDecision::RequireUserns {
            reason: format!(
                "{WORKER_DB_GUARD_ENV}={} is not verified read-only: {reason}",
                value.display()
            ),
        },
        WorkerRunMarker::ReadOnly(_) if !unknown.is_empty() => GuardDecision::RequireUserns {
            reason: format!("the protected set is unknown: {}", unknown.join("; ")),
        },
        WorkerRunMarker::ReadOnly(_) => GuardDecision::Exempt,
    }
}

/// ADR-0126 A1-2: 本番 config の既定の場所。`$HOME` ではなく `getpwuid(getuid())` の home を基準にする
/// （試験が `$HOME` を書き換える）。
pub fn production_config_path() -> Option<PathBuf> {
    nix::unistd::User::from_uid(nix::unistd::getuid())
        .ok()
        .flatten()
        .map(|u| u.dir.join(".config/celeris/config.toml"))
}

#[cfg(test)]
#[path = "db_guard_tests.rs"]
mod tests;
