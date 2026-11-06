import importlib.util
import sqlite3
import tempfile
import unittest
from pathlib import Path

spec = importlib.util.spec_from_file_location(
    "snapshot_db", Path(__file__).resolve().parents[1] / "snapshot_db.py")
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class SnapshotTests(unittest.TestCase):
    def test_concurrent_wal_write_keeps_original_snapshot(self):
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory) / "source #1.sqlite3"
            destination = Path(directory) / "copy.sqlite3"
            with sqlite3.connect(source) as writer:
                writer.execute("PRAGMA journal_mode=WAL")
                writer.execute("CREATE TABLE data (id INTEGER PRIMARY KEY, value BLOB)")
                writer.executemany("INSERT INTO data VALUES (?, zeroblob(4096))",
                                   [(i,) for i in range(600)])
                writer.commit()
                updates = []

                def progress(status, remaining, total):
                    if not updates:
                        self.assertGreater(remaining, 0)
                        writer.execute("DELETE FROM data WHERE id = 0")
                        writer.commit()
                        updates.append(True)

                module.snapshot(source, destination, progress=progress)
                self.assertEqual(updates, [True])
                self.assertEqual(writer.execute("SELECT count(*) FROM data").fetchone()[0], 599)
            writer.close()
            with sqlite3.connect(destination) as copy:
                self.assertEqual(copy.execute("SELECT count(*) FROM data").fetchone()[0], 600)
                self.assertEqual(copy.execute("PRAGMA integrity_check").fetchone()[0], "ok")
            copy.close()

    def test_missing_source_is_not_created(self):
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory) / "missing"
            with self.assertRaises(sqlite3.OperationalError):
                module.snapshot(source, Path(directory) / "copy")
            self.assertFalse(source.exists())


if __name__ == "__main__":
    unittest.main()
