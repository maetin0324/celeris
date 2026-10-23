"""Reference failures propagate through the mandatory workspace entrypoint."""
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]


class ArchitectureMapTests(unittest.TestCase):
    def test_reference_positive_and_negative(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / 'scripts/dev').mkdir(parents=True)
            (root / 'docs').mkdir()
            script = root / 'scripts/dev/check-architecture-map.py'
            shutil.copy(ROOT / 'scripts/dev/check-architecture-map.py', script)
            doc = root / 'docs/architecture-map.md'
            doc.write_text('| source | `scripts/dev/check-architecture-map.py` |\n')
            ok = subprocess.run(['python3', str(script)], capture_output=True, text=True)
            self.assertEqual(ok.returncode, 0, ok.stderr)
            doc.write_text('| source | `crates/missing.rs` |\n')
            bad = subprocess.run(['python3', str(script)], capture_output=True, text=True)
            self.assertEqual(bad.returncode, 1)
            self.assertIn('crates/missing.rs', bad.stderr)
            # A broken map must fail test-affected before cargo is even invoked.
            for name in ['check-health.sh', 'test-affected.sh']:
                shutil.copy(ROOT / 'scripts/dev' / name, root / 'scripts/dev' / name)
            (root / 'scripts/dev/check-source-size.py').write_text('raise SystemExit(0)\n')
            subprocess.run(['git', 'init', '-q', str(root)], check=True)
            subprocess.run(['git', '-C', str(root), '-c', 'user.name=test', '-c',
                            'user.email=test@example.com', 'commit', '--allow-empty', '-qm', 'initial'], check=True)
            result = subprocess.run(['bash', 'scripts/dev/test-affected.sh'], cwd=root,
                                    capture_output=True, text=True)
            self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
            self.assertIn('crates/missing.rs', result.stderr)


if __name__ == '__main__':
    unittest.main()
