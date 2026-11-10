"""Small isolated Git histories shared by Python and Rust health tests.

CLI: python3 -I scripts/tests/repo_health_fixtures.py build KIND VARIANT DEST
Only DEST is written; no compiler, network, or model is invoked.
"""
from __future__ import annotations

import argparse
from dataclasses import asdict, dataclass
import json
import os
from pathlib import Path
import subprocess

PROFILE = 'scripts/dev/repo-health/profiles/default.toml'
KINDS = ('ts', 'rust', 'multi')
VARIANTS = ('good', 'dup-contract', 'dup-state', 'dep-inversion',
            'security-double', 'vendor-generated')


def git(repo: Path, *args: str) -> str:
    # Do not inherit a caller's index, worktree, repository, or config injection.
    env = {k: v for k, v in os.environ.items() if not k.startswith('GIT_')}
    env.update(GIT_CONFIG_GLOBAL='/dev/null', GIT_CONFIG_NOSYSTEM='1',
               GIT_AUTHOR_DATE='2026-01-01T00:00:00Z',
               GIT_COMMITTER_DATE='2026-01-01T00:00:00Z')
    return subprocess.run(['git', '-C', str(repo), *args], env=env, check=True,
                          capture_output=True, text=True).stdout.strip()


def write(repo: Path, path: str, body: str) -> None:
    dest = repo / path
    dest.parent.mkdir(parents=True, exist_ok=True)
    dest.write_text(body, encoding='utf-8')


def commit(repo: Path, subject: str) -> str:
    git(repo, 'add', '-A')
    git(repo, '-c', 'core.hooksPath=/dev/null', 'commit', '-q', '-m', subject)
    return git(repo, 'rev-parse', 'HEAD')


@dataclass
class Fixture:
    base: str
    head: str
    repos: list[str]
    expected: list[str]


def contract(kind: str, name: str, fields=('id', 'owner', 'state')) -> str:
    if kind == 'ts':
        return f'export interface {name} {{\n' + ''.join(f'  {f}: string;\n' for f in fields) + '}\n'
    return f'pub struct {name} {{\n' + ''.join(f'    pub {f}: String,\n' for f in fields) + '}\n'


def profile(kind: str) -> str:
    prefix = 'src' if kind == 'ts' else 'crates'
    broker = r'^src/broker/verify\.ts$' if kind == 'ts' else r'^crates/broker/src/verify\.rs$'
    language = 'typescript' if kind == 'ts' else 'rust'
    body = f'''version = "1"
exclude_paths = ['^vendor/']
policy_paths = ['^scripts/dev/repo-health/', '^size-baseline\\.json$']
[adapters]
enabled = ["{language}", "markdown"]
'''
    for rid in ('policy.changed', 'dup.contract', 'dup.state', 'dup.public-surface', 'deps.cycle', 'deps.violation'):
        body += f'\n[rules."{rid}"]\nrequired = true\n'
    for layer, allow in (('core', []), ('auth', ['core']), ('app', ['core', 'auth'])):
        body += f'''\n[[rules."deps.violation".layers]]
name = "{layer}"
paths = ['^{prefix}/{layer}/']
allow = {json.dumps(allow)}
'''
    return body + f'''\n[[rules."dup.contract".exceptions]]
path = '{broker}'
reason = "security 境界の独立検証"
'''


def _single(kind: str, variant: str, repo: Path, with_profile: bool) -> Fixture:
    repo.mkdir(parents=True, exist_ok=True)
    git(repo, 'init', '-q', '-b', 'main')
    for key, value in (('user.name', 'Fixture'), ('user.email', 'fixture@example.invalid'),
                       ('commit.gpgsign', 'false')):
        git(repo, 'config', key, value)
    ext = 'ts' if kind == 'ts' else 'rs'
    def source(module: str, file: str) -> str:
        return f'src/{module}/{file}.{ext}' if kind == 'ts' else f'crates/{module}/src/{file}.{ext}'
    write(repo, 'README.md', '# Orchard sample\n')
    write(repo, 'docs/guide.md', '# Guide\n')
    write(repo, source('core', 'session'), contract(kind, 'Session'))
    write(repo, source('core', 'store'),
          'export const insertSession = "INSERT INTO sessions (id) VALUES (?)";\n' if kind == 'ts' else
          'pub const INSERT_SESSION: &str = "INSERT INTO sessions (id) VALUES (?)";\n')
    write(repo, source('auth', 'verify'), contract(kind, 'TokenProof', ('issuer', 'signature', 'subject')) +
          ('export function verifyToken(token: string) { return token.length > 0; }\n' if kind == 'ts' else
           'pub fn verify_token(token: &str) -> bool { !token.is_empty() }\n'))
    if kind == 'ts':
        write(repo, 'package.json', '{"name":"orchard","type":"module"}\n')
        write(repo, 'tsconfig.json', '{"compilerOptions":{"strict":true}}\n')
        write(repo, 'src/core/util.js', 'export const sessionLabel = value => value.id;\n')
        write(repo, 'src/app/index.ts', "import { sessionLabel } from '../core/util.js';\nexport const label = sessionLabel;\n")
    else:
        write(repo, 'Cargo.toml', '[workspace]\nmembers = ["crates/core", "crates/auth", "crates/app"]\nresolver = "2"\n')
        for module in ('core', 'auth', 'app'):
            manifest = f'[package]\nname = "orchard-{module}"\nversion = "0.1.0"\nedition = "2021"\n'
            if module == 'app':
                manifest += '[dependencies]\norchard-core = { path = "../core" }\n'
            write(repo, f'crates/{module}/Cargo.toml', manifest)
            mods = {'core': 'pub mod session;\npub mod store;\npub fn session_count() -> usize { 1 }\n',
                    'auth': 'pub mod verify;\n', 'app': 'pub fn count() -> usize { orchard_core::session_count() }\n'}
            write(repo, f'crates/{module}/src/lib.rs', mods[module])
    if with_profile:
        write(repo, PROFILE, profile(kind))
    base = commit(repo, 'fixture base')
    git(repo, 'switch', '-q', '-c', 'candidate')
    expected = []
    if variant == 'good':
        write(repo, source('app', 'summary'), contract(kind, 'Summary', ('total',)))
        if kind == 'ts':
            write(repo, 'src/core/count.ts', 'export function countSessions() { return 1; }\n')
            write(repo, 'src/app/index.ts', "import { countSessions } from '../core/count';\nexport const total = countSessions();\n")
        else:
            lib = repo / 'crates/core/src/lib.rs'
            lib.write_text(lib.read_text() + 'pub fn active_sessions() -> usize { 1 }\n')
            write(repo, 'crates/app/src/lib.rs', 'pub mod summary;\npub fn count() -> usize { orchard_core::active_sessions() }\n')
    elif variant == 'dup-contract':
        write(repo, source('app', 'session_view'), contract(kind, 'SessionView'))
        expected = [f'dup.contract:shape:{"typescript" if kind == "ts" else "rust"}:id,owner,state']
    elif variant == 'dup-state':
        write(repo, source('app', 'session_writer'),
              'export const updateSession = "UPDATE sessions SET state = ? WHERE id = ?";\n' if kind == 'ts' else
              'pub const UPDATE_SESSION: &str = "UPDATE sessions SET state = ? WHERE id = ?";\n')
        expected = ['dup.state:table:sessions']
    elif variant == 'dep-inversion':
        if kind == 'ts':
            write(repo, 'src/core/inverted.ts', "import { label } from '../app/index';\nexport const inverted = label;\n")
        else:
            path = repo / 'crates/core/Cargo.toml'
            path.write_text(path.read_text() + '[dependencies]\norchard-app = { path = "../app" }\n')
        expected = ['deps.violation:core->app']
    elif variant == 'security-double':
        write(repo, source('broker', 'verify'), contract(kind, 'BoundaryProof', ('issuer', 'signature', 'subject')) +
              ('export function checkBoundary(token: string) { return token.length > 0; }\n' if kind == 'ts' else
               'pub fn check_boundary(token: &str) -> bool { !token.is_empty() }\n'))
        expected = [f'dup.contract:shape:{"typescript" if kind == "ts" else "rust"}:issuer,signature,subject']
    elif variant == 'vendor-generated':
        write(repo, f'vendor/lib/types.{ext}', contract(kind, 'Session'))
        write(repo, source('generated', 'api'), '// @generated\n' + contract(kind, 'GeneratedSession'))
    head = commit(repo, 'fixture ' + variant)
    return Fixture(base, head, ['.'], expected)


def build(kind: str, variant: str, dest: Path, *, with_profile: bool = True) -> Fixture:
    if kind not in KINDS or variant not in VARIANTS:
        raise ValueError(f'unsupported fixture: {kind}/{variant}')
    dest = Path(dest).absolute()
    if dest.is_symlink() or (dest.exists() and (not dest.is_dir() or any(dest.iterdir()))):
        raise ValueError('destination must be absent or an empty directory')
    if kind != 'multi':
        return _single(kind, variant, dest, with_profile)
    fixtures = [_single(k, variant, dest / k, with_profile) for k in ('ts', 'rust')]
    # Multi consumers resolve main/candidate in each repo; these top-level SHAs
    # identify the first repo, as the report contract only has scalar SHA fields.
    return Fixture(fixtures[0].base, fixtures[0].head, ['ts', 'rust'],
                   [label for f in fixtures for label in f.expected])


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=['build'])
    parser.add_argument('kind', choices=KINDS)
    parser.add_argument('variant', choices=VARIANTS)
    parser.add_argument('dest', type=Path)
    args = parser.parse_args()
    print(json.dumps(asdict(build(args.kind, args.variant, args.dest))))


if __name__ == '__main__':
    main()
