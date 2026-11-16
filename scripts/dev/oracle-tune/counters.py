#!/usr/bin/env python3
"""Summarize captured Oracle counters using only the standard library."""
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
    return f'{value / denominator:.2f}' if denominator else 'n/a'


def main():
    directory = Path(sys.argv[1])
    results = {}
    for line in (directory / 'bench.jsonl').read_text().splitlines():
        row = json.loads(line)
        results[(f"{row['workload']}-{row['concurrency']}", str(row['run']))] = row['successes']
    lines = ['Per-request counters; numbers stay in .evidence/ (OTN).',
             '', 'Part 1: session counters',
             '\t'.join(('entry', 'run', 'successes', *STATS, 'logons_cumulative'))]
    for (entry, run), successes in results.items():
        stem = directory / f'snap-{entry}-run{run}'
        start, end = Path(f'{stem}-start.txt'), Path(f'{stem}-end.txt')
        if not (start.exists() and end.exists()):
            continue
        before, after = sessions(start), sessions(end)
        totals = {}
        for key, value in after.items():
            stat = key[2]
            totals[stat] = totals.get(stat, 0) + value - before.get(key, 0)
        lines.append('\t'.join((entry, run, str(successes),
                               *(rate(totals.get(stat, 0), successes) for stat in STATS),
                               str(totals.get('logons_cumulative', 0)))))
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
