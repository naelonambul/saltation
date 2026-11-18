#!/usr/bin/env python3
"""Exercise the real bank build stage with old-timestamp fixture sources."""
from pathlib import Path
import os
import shutil
import subprocess
import tempfile
import uuid

ROOT = Path(__file__).resolve().parents[2]
IMAGE = 'saltation/bank-dev:local'


def run(command, **options):
    return subprocess.run(command, check=True, text=True, **options)


def main():
    # Fixed, project-specific tiny cache; locked by the real Dockerfile RUN.
    # Tags and temporary source context belong only to this invocation.
    tag = 'saltation/cache-witness:' + uuid.uuid4().hex
    try:
        with tempfile.TemporaryDirectory(prefix='saltation-buildkit-witness-') as temp:
            root = Path(temp)
            dockerfile = (ROOT / 'bank/Dockerfile').read_text()
            assert 'cargo clean --release --package bank --locked' in dockerfile
            assert 'sharing=locked' in dockerfile
            assert 'id=saltation-bank-build-guarded-v1,' in dockerfile
            dockerfile = dockerfile.replace('id=saltation-bank-build-guarded-v1,', 'id=saltation-bank-build-witness-v1,')
            for index, label in enumerate(('A', 'B', 'A')):
                # Distinct worktree contexts are essential: reusing one context
                # and restoring identical stat data can make Docker's local
                # file-sync cache send the old source before RUN even starts.
                context = root / ('worktree-' + str(index))
                (context / 'src').mkdir(parents=True)
                (context / 'Dockerfile').write_text(dockerfile)
                shutil.copyfile(ROOT / 'bank/.dockerignore', context / '.dockerignore')
                (context / 'Cargo.toml').write_text('[package]\nname="bank"\nversion="0.1.0"\nedition="2024"\n')
                (context / 'Cargo.lock').write_text('version = 4\n\n[[package]]\nname = "bank"\nversion = "0.1.0"\n')
                source = context / 'src/main.rs'
                body = 'fn main(){println!("SOURCE_%s");}\n' % label
                source.write_text(body)
                os.utime(source, (946684800, 946684800))
                (context / 'context_nonce').write_text(str(index))
                for entry in [*context.rglob('*'), context]:
                    os.utime(entry, (946684800, 946684800))
                run(['docker', 'build', '--progress', 'plain', '--target', 'build', '-t', tag, str(context)])
                copied = run(['docker', 'run', '--rm', '--entrypoint', 'cat', tag, '/src/src/main.rs'], capture_output=True)
                assert copied.stdout == body, 'Docker context did not deliver the requested source'
                result = run(['docker', 'run', '--rm', '--entrypoint', '/bank', tag], capture_output=True)
                assert result.stdout.strip() == 'SOURCE_' + label, result.stdout
                print('PASS BuildKit old-timestamp source ' + label, flush=True)
    finally:
        subprocess.run(['docker', 'image', 'rm', tag], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


if __name__ == '__main__':
    main()
