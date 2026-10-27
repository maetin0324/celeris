#!/usr/bin/env bash
# Shared operation audit. Sourced by promote/rollback; Python is required before side effects.
sd_audit() {
  python3 - "$@" <<'PY'
import datetime
import json
import os
from pathlib import Path
import sys
import uuid


def now():
    return datetime.datetime.now(datetime.timezone.utc).isoformat()


def context():
    result = {}
    for key in ('actor', 'reason', 'policy_ref'):
        value = os.environ.get('SD_' + key.upper(), '')
        if not value.strip():
            raise ValueError('SD_' + key.upper() + ' is required')
        result[key] = value
    actor = result['actor']
    if actor not in ('human', 'cos') and not any(actor.startswith(p) and actor[len(p):].strip() for p in ('operator:', 'system:')):
        raise ValueError('SD_ACTOR must identify human, cos, operator:<name> or system:<name>')
    for key in ('task_refs', 'decision_refs'):
        value = json.loads(os.environ.get('SD_' + key.upper(), '[]'))
        if not isinstance(value, list) or any(not isinstance(x, str) or not x.strip() for x in value):
            raise ValueError('SD_' + key.upper() + ' must be a JSON array of nonempty strings')
        result[key] = value
    return result


def write(path, value):
    tmp = path.with_suffix('.tmp')
    tmp.write_text(json.dumps(value, ensure_ascii=False, indent=2) + '\n')
    tmp.replace(path)


try:
    command = sys.argv[1]
    if command == 'validate':
        context()
    elif command == 'begin':
        directory, operation, source, target, mode = sys.argv[2:]
        if operation not in ('promote', 'rollback'):
            raise ValueError('invalid deployment operation')
        root = Path(directory)
        data = context()
        verify = root / 'verify.json'
        data.update(operation=operation, mode=mode, **{'from': source or None}, to=target,
                    started_at=now(), status='started', exit_code=None,
                    verify=json.loads(verify.read_text()) if verify.exists() else None)
        audit = root / 'audit'
        audit.mkdir(exist_ok=True)
        path = audit / (uuid.uuid4().hex + '.json')
        write(path, data)
        print(path)
    elif command == 'finish':
        path = Path(sys.argv[2])
        code = int(sys.argv[3])
        data = json.loads(path.read_text())
        data.update(status='succeeded' if code == 0 else 'failed', exit_code=code, finished_at=now())
        write(path, data)
        if code == 0:
            root = path.parent.parent
            names = ['promoted.json'] if data['operation'] == 'promote' else ['rolled_back.json']
            if data['operation'] == 'rollback' and data['mode'] != 'restore-db':
                names.append('promoted.json')
            for name in names:
                dest = root / name
                pending = Path(str(path) + '.summary')
                summary = json.loads(pending.read_text()) if pending.exists() else {}
                summary.update(data)
                write(dest, summary)
        Path(str(path) + '.summary').unlink(missing_ok=True)
    else:
        raise ValueError('unknown audit command')
except (ValueError, OSError) as error:
    print('deployment audit: ' + str(error), file=sys.stderr)
    sys.exit(1)
PY
}
