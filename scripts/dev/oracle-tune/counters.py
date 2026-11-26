import json
from pathlib import Path
import re
import sys

STATS = ('user_commits', 'user_rollbacks', 'redo_synch_writes',
         'SQL*Net_roundtrips_to/from_client', 'execute_count', 'user_calls')


def sessions(path):
    rows = {}
    for line in path.read_text().splitlines():
        if line.startswith('sst '):
            _, sid, serial, stat, value = line.split()
            rows[(sid, serial, stat)] = int(value)
    return rows


def rate(value, denominator):
    if value is None:
        return '?'
    return f'{value / denominator:.2f}' if denominator else 'n/a'


def session_delta(before, after):
    if not before or before.keys() != after.keys():
        return None
    totals = {}
    for key, value in after.items():
        delta = value - before[key]
        if delta < 0:
            return None
        totals[key[2]] = totals.get(key[2], 0) + delta
    return totals


def database_delta(start, end):
    def snapshot(path):
        startup, stats = None, {}
        for line in path.read_text().splitlines():
            if line.startswith('startup '):
                startup = line.split()[1]
            elif line.startswith('sys '):
                _, name, value = line.split()
                stats[name] = int(value)
        return startup, stats
    startup, before = snapshot(start)
    next_startup, after = snapshot(end)
    if startup is None or startup != next_startup or before.keys() != after.keys():
        return None
    delta = {name: value - before[name] for name, value in after.items()}
    return None if any(value < 0 for value in delta.values()) else delta


def main():
    directory = Path(sys.argv[1])
    results = {}
    for line in (directory / 'bench.jsonl').read_text().splitlines():
        row = json.loads(line)
        results[(f"{row['workload']}-{row['concurrency']}", str(row['run']))] = row['successes']
    lines = ['Per-request counters; numbers stay in .evidence/ (OTN).',
             '', 'Part 1: surviving-session counters; ? means turnover or invalid sample',
             '\t'.join(('entry', 'run', 'successes', *STATS, 'logons_cumulative'))]
    lifetime = []
    for (entry, run), successes in results.items():
        stem = directory / f'snap-{entry}-run{run}'
        start, end = Path(f'{stem}-start.txt'), Path(f'{stem}-end.txt')
        if not (start.exists() and end.exists()):
            continue
        before, after = sessions(start), sessions(end)
        totals = session_delta(before, after)
        lines.append('\t'.join((entry, run, str(successes),
                               *(rate(totals.get(stat) if totals is not None else None, successes) for stat in STATS),
                               str(totals.get('logons_cumulative', '?')) if totals is not None else '?')))
        total = database_delta(start, end)
        lifetime.append('\t'.join((entry, run, str(successes),
                                   *(rate(total.get(stat) if total is not None else None, successes) for stat in STATS),
                                   str(total.get('logons_cumulative', '?')) if total is not None else '?')))
    lines.extend(('', 'Part 1b: database-lifetime counters in the snapshot container (may include diagnostic/background work)',
                  'Retired sessions remain included. Do not attribute these totals to one schema.',
                  'Observer calls/commits can contribute; do not subtract assumed diagnostic logon counts.',
                  'Use the same isolated capture protocol on both sides; raw logons are not schema-filtered.',
                  '\t'.join(('entry', 'run', 'successes', *STATS, 'logons_cumulative')),
                  *lifetime))
    lines.extend(('', 'Part 2: cursor counters'))
    for path in sorted(directory.glob('sqldelta-*-run*.txt')):
        match = re.fullmatch(r'sqldelta-(.+)-run(\d+)\.txt', path.name)
        if not match:
            continue
        entry, run = match.groups()
        successes = results.get((entry, run), 0)
        lines.extend(('', f'{entry} run {run}',
                      'statement\tsql_id\tchild\tstatus\texecutions/request\tbuffer_gets/execution'))
        for line in path.read_text().splitlines():
            fields = line.split(None, 9)
            if len(fields) != 10 or fields[2] not in {'ok', 'new', 'reloaded', 'reset', 'missing'}:
                continue
            sql_id, child, status, executions, gets, _, _, _, _, sql = fields
            if status in {'ok', 'new'} and int(executions) == 0:
                continue
            label = sql.split('-- ', 1)[1].split(':', 1)[0] if '-- ' in sql else sql[:40]
            per_request, per_execution = '?', '?'
            if status in {'ok', 'new'}:
                per_request = rate(int(executions), successes)
                per_execution = rate(int(gets), int(executions))
            lines.append('\t'.join((label, sql_id, child, status, per_request, per_execution)))
    (directory / 'counters.txt').write_text('\n'.join(lines) + '\n')


if __name__ == '__main__':
    main()
