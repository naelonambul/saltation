#!/usr/bin/env python3
"""Budget a new guarded cache without deleting or changing legacy artifacts."""
import json
import os
from pathlib import Path
import subprocess
import sys


def allocated(path):
    if not path.exists():
        return 0
    return int(subprocess.check_output(['du', '-sk', str(path)], text=True).split()[0]) * 1024


def main():
    if sys.argv[1:] == ['record']:
        record = json.load(sys.stdin)
        Path('/target/.saltation-capacity-v1.json').write_text(json.dumps(record) + '\n')
        return 0
    legacy = Path('/legacy')
    storage = legacy if legacy.exists() else Path('/registry')
    # The historical cache bounds the new target; reserve another release tree
    # for the separately named BuildKit cache, plus filesystem headroom.
    historical = allocated(legacy)
    release = allocated(legacy / 'release') if legacy.exists() else 0
    budget = max(historical + release, 8 * 1024**3) + 2 * 1024**3
    fs = os.statvfs(storage)
    free = fs.f_bavail * fs.f_frsize
    if free < budget:
        print('rust-cache: insufficient disk budget for new guarded caches; '
              f'need {budget} bytes, available {free}; legacy caches left untouched', file=sys.stderr)
        return 2
    print(json.dumps({'format': 1, 'budget_bytes': budget, 'available_bytes': free,
                      'legacy_bytes': historical, 'legacy_release_bytes': release}))
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
