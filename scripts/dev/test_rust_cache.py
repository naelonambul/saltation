import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import unittest

RUNNER = Path(__file__).with_name('rust-cache.py')


class Freshness(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix='saltation-rust-cache-')
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.bank = self.root / 'bank'
        (self.bank / 'src').mkdir(parents=True)
        self.contracts = self.root / 'contracts'
        self.contracts.mkdir()
        dep = self.root / 'support'
        (dep / 'src').mkdir(parents=True)
        (dep / 'Cargo.toml').write_text('[package]\nname="cache_support"\nversion="0.1.0"\nedition="2024"\n')
        (dep / 'src/lib.rs').write_text('pub fn suffix()-> &\'static str { "dep" }\n')
        (self.bank / 'Cargo.toml').write_text('[package]\nname="bank"\nversion="0.1.0"\nedition="2024"\n[dependencies]\ncache_support={path="../support"}\n')
        self.env = dict(os.environ, CARGO_TARGET_DIR=str(self.root / 'target'))
        self.source('A')
        self.cargo('generate-lockfile', '--offline')

    def source(self, label, fail=False, wait=False):
        source = ('fn main(){println!("%s:{}",cache_support::suffix());}\n'
                  '#[test] fn witness(){%s assert!(%s);}\n') % (
                      label,
                      'std::fs::write(std::env::var("CACHE_READY").unwrap(),"ready").unwrap(); std::thread::sleep(std::time::Duration::from_secs(30));' if wait else '',
                      'false' if fail else 'true')
        p = self.bank / 'src/main.rs'
        p.write_text(source)
        os.utime(p, (946684800, 946684800))

    def cargo(self, *args):
        p = subprocess.run(['cargo', *args], cwd=self.bank, env=self.env, text=True, capture_output=True)
        self.assertEqual(p.returncode, 0, p.stderr)
        return p

    def command(self, profile='debug', verb='run'):
        return [sys.executable, str(RUNNER), '--bank', str(self.bank), '--contracts', str(self.contracts), '--profile', profile, '--', 'cargo', verb, '--locked', '--offline', *(['--release'] if profile == 'release' else [])]

    def run_guard(self, label, profile='debug', verb='run', code=0):
        p = subprocess.run(self.command(profile, verb), env=self.env, text=True, capture_output=True)
        self.assertEqual(p.returncode, code, p.stdout + p.stderr)
        if verb == 'run' and code == 0:
            self.assertEqual(p.stdout.strip(), label + ':dep')
        return p

    def test_old_timestamps_debug_and_release(self):
        for profile in ('debug', 'release'):
            for label in ('A', 'B', 'A'):
                self.source(label)
                self.run_guard(label, profile)

    def test_debug_success_does_not_bless_release(self):
        self.run_guard('A', 'release')
        self.source('B')
        self.run_guard('B', 'debug')
        self.run_guard('B', 'release')
        self.source('A')
        self.run_guard('A', 'debug')
        self.run_guard('A', 'release')

    def test_unchanged_reuses_bank_and_preserves_dependency(self):
        self.run_guard('A')
        bank = self.root / 'target/debug/bank'
        deps = list((self.root / 'target/debug/deps').glob('libcache_support-*.rlib'))
        self.assertTrue(deps)
        stamps = {p: p.stat().st_mtime_ns for p in [bank, *deps]}
        self.run_guard('A')
        self.assertEqual(stamps, {p: p.stat().st_mtime_ns for p in stamps})
        self.source('B')
        self.run_guard('B')
        self.assertEqual({p: stamps[p] for p in deps}, {p: p.stat().st_mtime_ns for p in deps})

    def test_failed_b_then_return_to_a(self):
        for profile in ('debug', 'release'):
            self.source('A')
            self.run_guard('A', profile, 'test')
            self.source('B', fail=True)
            self.run_guard('B', profile, 'test', 101)
            self.source('A')
            self.run_guard('A', profile, 'test')

    def test_cancelled_b_then_return_to_a(self):
        self.run_guard('A', verb='test')
        self.source('B', wait=True)
        ready = self.root / 'ready'
        env = dict(self.env, CACHE_READY=str(ready))
        p = subprocess.Popen(self.command(verb='test'), env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        try:
            deadline = time.monotonic() + 20
            while not ready.exists() and p.poll() is None and time.monotonic() < deadline:
                time.sleep(.05)
            self.assertTrue(ready.exists(), 'test must compile and enter before cancellation')
            p.terminate()
            p.communicate(timeout=10)
            self.assertNotEqual(p.returncode, 0)
        finally:
            if p.poll() is None:
                p.kill()
                p.communicate()
        self.source('A')
        self.run_guard('A', verb='test')

    def test_concurrent_commands_wait_for_running_tests(self):
        self.source('A', wait=True)
        ready = self.root / 'ready'
        first = subprocess.Popen(self.command(verb='test'), env=dict(self.env, CACHE_READY=str(ready)), stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        second = None
        try:
            deadline = time.monotonic() + 20
            while not ready.exists() and first.poll() is None and time.monotonic() < deadline:
                time.sleep(.05)
            self.assertTrue(ready.exists())
            second = subprocess.Popen(self.command(), env=self.env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
            time.sleep(.3)
            self.assertIsNone(second.poll(), 'cache lock must cover test execution, not just compilation')
            first.terminate()
            first.communicate(timeout=10)
            out, err = second.communicate(timeout=20)
            self.assertEqual(second.returncode, 0, out + err)
            self.assertEqual(out.strip(), 'A:dep')
        finally:
            for proc in (first, second):
                if proc is not None and proc.poll() is None:
                    proc.kill()
                    proc.communicate()

    def test_contract_change_invalidates_cached_package(self):
        self.run_guard('A')
        artifact = self.root / 'target/debug/bank'
        stamp = artifact.stat().st_mtime_ns
        (self.contracts / 'sample.json').write_text('{}')
        self.run_guard('A')
        self.assertNotEqual(stamp, artifact.stat().st_mtime_ns)

    def test_alternating_test_targets_reuse_cache_and_source_change_invalidates(self):
        tests_dir = self.bank / 'tests'
        tests_dir.mkdir()
        (tests_dir / 'alpha.rs').write_text('#[test] fn a() { assert!(true); }\n')
        (tests_dir / 'beta.rs').write_text('#[test] fn b() { assert!(true); }\n')
        first = subprocess.run(
            [*self.command(verb='test'), '--test', 'alpha', '--', '--nocapture'],
            env=self.env, text=True, capture_output=True,
        )
        self.assertEqual(first.returncode, 0, first.stdout + first.stderr)
        self.assertIn('invalidated=yes', first.stderr)
        alpha_bins = list((self.root / 'target/debug/deps').glob('alpha-*'))
        self.assertTrue(alpha_bins)
        stamps = {p: p.stat().st_mtime_ns for p in alpha_bins if p.is_file()}
        second = subprocess.run(
            [*self.command(verb='test'), '--test', 'beta', '--', '--nocapture'],
            env=self.env, text=True, capture_output=True,
        )
        self.assertEqual(second.returncode, 0, second.stdout + second.stderr)
        self.assertIn('invalidated=no', second.stderr)
        self.assertEqual(stamps, {p: p.stat().st_mtime_ns for p in stamps})
        self.source('B')
        third = subprocess.run(
            [*self.command(verb='test'), '--test', 'beta', '--', '--nocapture'],
            env=self.env, text=True, capture_output=True,
        )
        self.assertEqual(third.returncode, 0, third.stdout + third.stderr)
        self.assertIn('invalidated=yes', third.stderr)
        fourth_env = dict(self.env, CARGO_BUILD_JOBS='2')
        fourth = subprocess.run(
            [*self.command(verb='test'), '--bins', '--', '--nocapture'],
            env=fourth_env, text=True, capture_output=True,
        )
        self.assertEqual(fourth.returncode, 0, fourth.stdout + fourth.stderr)
        self.assertIn('invalidated=no', fourth.stderr)


if __name__ == '__main__':
    unittest.main()
