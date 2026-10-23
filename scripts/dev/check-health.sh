#!/usr/bin/env bash
# Mandatory lightweight checks; full tests remain release-only.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
echo "health: candidate=$(git rev-parse HEAD) (working tree included)"
python3 scripts/dev/check-source-size.py
python3 scripts/dev/check-architecture-map.py
sh scripts/dev/check-doc-links.sh
sh scripts/dev/check-adr-numbers.sh
sh scripts/dev/progress-index.sh --check
sh scripts/dev/tests/progress_index_commit_hashes.sh
python3 -m unittest discover -s scripts/tests -p 'test_source_size*.py'
python3 -m unittest discover -s scripts/tests -p 'test_architecture_map.py'
sh scripts/dev/check-doc-links.sh --self-test
