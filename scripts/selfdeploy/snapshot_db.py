#!/usr/bin/env python3
"""Copy a read-only SQLite snapshot without restarting on concurrent WAL writes."""
import sqlite3
import sys
from pathlib import Path


def snapshot(source, destination, *, progress=None):
    uri = Path(source).resolve().as_uri() + "?mode=ro"
    src = sqlite3.connect(uri, uri=True)
    try:
        src.execute("BEGIN")
        src.execute("SELECT count(*) FROM sqlite_schema").fetchone()
        dst = sqlite3.connect(destination)
        try:
            src.backup(dst, pages=256, progress=progress)
        finally:
            dst.close()
    finally:
        src.close()


if __name__ == "__main__":
    if len(sys.argv) != 3:
        sys.exit("usage: snapshot_db.py SOURCE DESTINATION")
    snapshot(sys.argv[1], sys.argv[2])
