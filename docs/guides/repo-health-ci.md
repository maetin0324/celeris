---
tasks: [01M6TYVJSWEY3ZE50S02ETYDNM]
---
# repo の CI から repo health を呼ぶ導入手順

repo の CI に `scripts/dev/repo-health/ci.sh` を足すと、各 PR（GitLab では merge request）の commit を、Celeris の review・統合と同じ検査器で検査できる。この guide はその手順を書く。

- CI は `sh scripts/dev/repo-health/ci.sh --base <base> --head <head> --out <report.json>` を呼ぶ。base は PR の行き先の branch と head の merge-base にする。
- exit 0 は合格、1 は必須 rule の fail、2 は検査できなかったこと。2 を合格に読み替えない。
- 検査器は base の commit の版を走らせる。Celeris も base の版を使う。PR の版を走らせると、検査器を書き換えた PR が自分を合格にできる。
- この check を merge の必須条件にする設定は、repo の持ち主が forge で行う。Celeris は CI の外の変更（main への直接 push など）を止められない（[強制できる範囲](#強制できる範囲)）。

検査器の契約（入力・rule の状態・exit code・report の形）の正本は [repo health の契約](../../agent-docs/adr/2026-11-07-repo-health-contract.md)、CI の入口の位置づけは [G3 の ADR](../../agent-docs/adr/2026-11-08-health-enforcement-loop.md) の (5)。ci.sh の引数は `sh scripts/dev/repo-health/ci.sh --help` が正本。

## 検査器を repo に置く
CI が走る環境には git と python3（3.11 以上。profile の読み込みに `tomllib` を使う）が要る。generic の profile では、ほかの tool は要らない。

### 写すもの
- `scripts/dev/repo-health/` のうち `profiles/` を除いた全部（`ci.sh`・`repo_health.py`・`validate_report.py`・`health/`・`rules/`・`schema/`）。
- `scripts/dev/source-size-report.py`。必須 rule の `deps.cycle` と `size.responsibility-growth` が行の分類に使う。無いと 2 つが `unsupported`（`tool_missing:source-size-report.py`）になり、exit 2 になる。
- 置く path は Celeris の repo と同じにする。検査器は repo の profile を `scripts/dev/repo-health/profiles/` で探す。
- `profiles/` を写さないのは、Celeris の `profiles/default.toml` が `extends = "celeris"` で Celeris 専用の上限（crate の層・地図・既存検査の command）を当てるため。

対象の repo の root で、Celeris の checkout（`<celeris>`）の main から写す例:

```sh
git -C <celeris> archive main scripts/dev/repo-health scripts/dev/source-size-report.py | tar -x
rm -rf scripts/dev/repo-health/profiles
git add scripts/dev
git commit -m "repo health の検査器を入れる（Celeris main $(git -C <celeris> rev-parse --short main) から）"
```

### 組み込み generic で始める
- repo に `scripts/dev/repo-health/profiles/default.toml` が無ければ、検査器は組み込み generic（`health/profile.py` の `BUILTIN_GENERIC`）で評価する。
- generic で評価する必須 rule は `deps.cycle`・`size.responsibility-growth`・`policy.changed`。`deps.violation`（層の宣言が要る）・`arch-map.*`（地図の path が要る）・既存検査の包み（`doc.links` など。command が要る）は `not_applicable` になり、exit code に効かない。
- 必須でない候補 rule（`dup.*`・`link.unlinked`・`migration.remnant`）は finding を report に残すだけで、exit code に効かない。
- adapter がある言語は rust・python・shell・typescript・markdown。ほかの言語（Go・Java・C など）の file を変える PR では、構造 rule が `unsupported`（`adapter_missing:<言語>`）になり exit 2 になる。その言語を構造 rule の対象から外すときは、下の profile に `ignore_languages` を書く。

### profile を作る
profile の項目の正本は `scripts/dev/repo-health/health/profile.py` の docstring。既存検査を包む書き方は Celeris の [profiles/celeris.toml](../../scripts/dev/repo-health/profiles/celeris.toml) にある。最小の例:

```toml
# scripts/dev/repo-health/profiles/default.toml
version = "1"
description = "この repo の既定"

[adapters]
# Go には adapter が無い。構造 rule の対象から外す（外す理由をここに書く）
ignore_languages = ["go"]
```

- `version` は `"1"` にする。ci.sh は `--policy-version 1` を渡す。base の profile の `version` が違えば、検査器は report を書かずに exit 2 で終わる。
- profile と検査器は policy の file（generic の `policy_paths` は `^scripts/dev/repo-health/`）。評価には base の commit の内容を使う。

### 導入の PR と、検査器・profile の更新
- 検査器を足す PR は、policy の file を足すので `policy.changed` の fail で exit 1 になる。base の版を走らせる下の設定例では、base に ci.sh が無いので exit 2 になる。どちらも想定どおりの結果で、repo の持ち主がこの PR を review して merge する。必須の check にするのは merge の後にする。
- profile を初めて足す PR は、base に profile が無いので bootstrap（report の `policy.source` が `bootstrap`）になる。検査器は head の profile で評価し、`policy.changed` を warning にする（exit 0）。
- その後に検査器か profile を変える PR は `policy.changed` の fail で exit 1 になる。policy の file だけを変えた PR でも exit 1 になる（Celeris の repo の `check-health.sh` が policy の file だけの変更を exit 0 にする扱い（[ADR health-review-integration](../../agent-docs/adr/2026-11-07-health-review-integration.md) の D6.2）は ci.sh には無い）。policy の変更だけの PR にし、repo の持ち主が review して merge する。

## CI での呼び方
### base の決め方
- 検査器は merge-base を自分で計算しない（契約 D1）。CI が base を決めて `--base` に渡す。
- PR: 行き先の branch と head の merge-base。GitHub Actions では `git merge-base "origin/$BASE_REF" HEAD`（`BASE_REF` は `github.base_ref`）、GitLab CI の merge request の pipeline では `CI_MERGE_REQUEST_DIFF_BASE_SHA`。
- main への push も検査するなら、base は push の前の先端（GitHub `github.event.before`、GitLab `CI_COMMIT_BEFORE_SHA`）。branch を初めて push したときなど、値が 0 の並びのときは base に使えない。
- head は `git rev-parse HEAD` の sha を渡す。ci.sh は commit を検査し、作業ツリーの未 commit の file は見ない。GitHub Actions の pull_request の checkout は、PR を行き先に merge した commit になる。

### 履歴の深さ
- base の commit が clone に無いと、ci.sh は exit 2 で終わる。report の `snapshot.shallow` が true、`snapshot.base.reachable` が false になり、`git fetch --deepen=<n>` か `git fetch --unshallow` の手順を stderr に出す。
- 設定例は全履歴を取る（GitHub の `actions/checkout` の `fetch-depth: 0`、GitLab の `GIT_DEPTH: "0"`）。merge-base は head の祖先なので、全履歴を取れば必ず届く。深さを絞るなら、head から base までの commit 数以上にする。

### exit code の意味
| exit | 意味 | CI での扱い |
|---|---|---|
| 0 | 必須 rule が全部 pass | 合格 |
| 1 | 必須 rule に fail がある（`policy.changed` を含む） | 不合格。report の finding を直す |
| 2 | 必須 rule に error・unknown・unsupported がある。report を書けない・契約に合わない、使い方の誤り、shallow clone で base に届かない | 不合格。stderr と report の `reason` を見て、環境か入力を直してから再実行する |

- fail と error が両方あれば exit 2 になる（契約 D3）。
- `not_applicable` の rule と、必須でない rule は exit code に効かない。

### report の置き場所
- `--out` に CI の作業 dir の下の path を渡し、exit code が 0 以外でも artifact として残す。既定の書き先 `${TMPDIR:-/tmp}/repo-health-ci.<pid>.json` は job の後に残らない。
- ci.sh は stdout に rule ごとの要約と `ci.sh: report=<path>` を出す。
- report の形は `celeris.repo-health.report/1`（schema は `scripts/dev/repo-health/schema/report-v1.schema.json`）。ci.sh は report を `validate_report.py` で確かめてから exit code を返す。

### base の検査器を走らせる
- Celeris の review は base の commit の検査器を一時 dir に取り出して走らせ、候補の検査器を使わない（`crates/task-dispatch/src/health_review/policy.rs` の `with_base_checker`）。
- CI では `git worktree add --detach <dir> <base>` で base を別の作業ツリーに出し、その ci.sh に `--head <head の sha>` を渡す。作業ツリーは object を共有するので、base の ci.sh から head の commit を読める。
- checkout の ci.sh をそのまま呼ぶと、ci.sh を書き換えた PR が自分を合格にできる。一時 repo で、先頭に `exit 0` を足した ci.sh は exit 0 を返した。同じ head を base の ci.sh で検査すると `policy.changed` の fail で exit 1 になった。

## 設定例
下の 2 つは文書の中の例で、この repo には `.github/` も `.gitlab-ci.yml` も置かない。shell の部分（base の作業ツリー、ci.sh の呼び方、exit code）は一時 repo で確かめた。GitHub Actions と GitLab CI の上では動かしていない（未確認）。

### GitHub Actions
```yaml
# .github/workflows/repo-health.yml
name: repo-health
on:
  pull_request:
jobs:
  repo-health:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
        with:
          fetch-depth: 0  # 全履歴を取り、base に届かせる
      - name: repo health
        env:
          BASE_REF: ${{ github.base_ref }}
        run: |
          set -eu
          base=$(git merge-base "origin/$BASE_REF" HEAD)
          head=$(git rev-parse HEAD)
          dir="$(mktemp -d)/base"
          git worktree add --detach "$dir" "$base"
          if [ ! -f "$dir/scripts/dev/repo-health/ci.sh" ]; then
            echo "base $base に scripts/dev/repo-health/ci.sh が無い" >&2
            exit 2
          fi
          sh "$dir/scripts/dev/repo-health/ci.sh" --base "$base" --head "$head" \
            --out "$GITHUB_WORKSPACE/repo-health-report.json"
      - name: report を残す
        if: always()
        uses: actions/upload-artifact@v4
        with:
          name: repo-health-report
          path: repo-health-report.json
          if-no-files-found: ignore
```

### GitLab CI
```yaml
# .gitlab-ci.yml
repo-health:
  image: python:3.12  # python3 3.11 以上と git が要る
  variables:
    GIT_DEPTH: "0"  # 全履歴を取り、base に届かせる
  rules:
    - if: $CI_PIPELINE_SOURCE == "merge_request_event"
  script:
    - |
      set -eu
      base="$CI_MERGE_REQUEST_DIFF_BASE_SHA"
      head=$(git rev-parse HEAD)
      dir="$(mktemp -d)/base"
      git worktree add --detach "$dir" "$base"
      if [ ! -f "$dir/scripts/dev/repo-health/ci.sh" ]; then
        echo "base $base に scripts/dev/repo-health/ci.sh が無い" >&2
        exit 2
      fi
      sh "$dir/scripts/dev/repo-health/ci.sh" --base "$base" --head "$head" \
        --out "$CI_PROJECT_DIR/repo-health-report.json"
  artifacts:
    when: always
    paths:
      - repo-health-report.json
```

## Celeris の review・統合との関係
- 検査器は同じ: Celeris の WorkUnit の check・段の統合・task の review も `repo_health.py` を base の版で走らせる（G3 の ADR の D1.3・D1.4）。base に検査器が無い repo には、Celeris は release に同梱した検査器を使う。CI には同梱の検査器が無いので、repo に検査器を置く。
- report の形と exit code は同じ: どちらも `celeris.repo-health.report/1` を書き、exit 0・1・2 の意味も同じ（契約 D3）。
- Celeris は CI の結果を自分の証跡にしない（G3 の ADR の D5.3）。CI が検査した commit と、Celeris が統合する候補の commit が同じだと Celeris は確かめられないので、Celeris は候補を自分で検査する。

## 強制できる範囲
- Celeris が止められるのは、Celeris が統合する経路（WorkUnit の check・段の統合・task の review・delivery の merge candidate など。G3 の ADR の D1.2）を通る変更だけ。
- 人や外部の道具が main に直接 push した変更や、CI を通さずに forge の画面で merge・編集した変更を、Celeris は止められない。CI に ci.sh を足しただけでも止まらない。
- 止めるには、repo の持ち主（forge の管理者）が branch 保護を設定する。GitHub では branch protection か ruleset で、この job を merge 前に通るべき status check にし、直接の push を許さない。GitLab では merge request の設定で pipeline の成功を merge の条件にし、protected branch で直接の push を許さない。この設定は Celeris の config にも Celeris の操作にも無い。
- 直接 push で入った負債は、G3 の ADR の (6) の棚卸し（cron の job。既定では無効）が後から見つける。
