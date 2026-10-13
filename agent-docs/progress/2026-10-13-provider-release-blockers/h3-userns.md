# H3 USERNS gate: long TMPDIR breaks Unix socket fixtures

## Cause

`test-parallel.sh` nested its temporary root below the inherited `$TMPDIR`, then exported `<root>/tmp` to every test. A deep inherited path made tempfile roots plus fixture subdirectories and names such as `celeris-credentiald/control.sock` exceed Linux Unix-domain socket `sun_path` (108 bytes). `UnixListener::bind` reports `InvalidInput: path must be shorter than SUN_LEN`; fixture helpers that hide broker startup errors turn it into a readiness timeout or `503 credential broker is not configured`.

`browser_h3_injection` had this dependency directly: its credentiald runtime was under `admin_env()`'s tempfile directory. Its control and resolve sockets now live under a short `/tmp/h3-*` directory, while DB, broker key/vault/audit, and browser fixture data remain in the isolated test tempdir. The fixture holds the short directory alive throughout the test.

Other affected tempfile-backed socket fixtures found during the USERNS workspace run include `celeris` browser-doctor, `celeris-credentiald` broker/injection/installed-unit, `task-api` browser_live_cross, and `celerisctl` browser approve socket tests.

## Reproduction

The specified command using `/local/prb` could not start because that directory is read-only in this run. The same long path shape was reproduced under the writable worktree:

```sh
d=$(mktemp -d "$PWD/.h3-tmp.XXXXXX")
deep=$d/celeris-test-parallel.aaaaaaaaaaaa/tmp/nested-for-long-socket-path
mkdir -p "$deep"
CELERIS_USERNS_TESTS=1 TMPDIR="$deep" cargo nextest run -p task-api --test browser_h3_injection
```

Before the fix this shape produced the reported readiness timeout (60 seconds). After the fix this run passed fixture readiness and advanced to the H3 isolation step, then failed immediately with `unshare: unshare failed: Operation not permitted`. `unshare -Ur true` is unavailable in this host environment, so the H3 end-to-end assertion cannot finish here; repeat on a user-namespace-capable host.

The USERNS workspace run exposed the same root cause directly in socket fixtures with `path must be shorter than SUN_LEN` panics. The final gate exited 100 (5001 passed, 34 failed, 13 skipped; `userns:false`). Besides the H3 case, it reported an unrelated `e2e::worker_db_read_only` failure whose output says `run_db=/local/celeris/data/db/celeris.sqlite3` and `raw_db_write_exit=0` / `raw_wal_write_exit=0`; this must be investigated by an owner before rerunning the full suite because it names the live DB path.

## Fix

`test-parallel.sh` creates its test temporary root under `/tmp/celeris-test-parallel.*`, independent of inherited `$TMPDIR`. The H3 fixture also locates its credentiald socket runtime under `/tmp/h3-*`. No retry or skip was added. Full USERNS verification remains to be rerun on a host where `unshare -Ur true` succeeds.
