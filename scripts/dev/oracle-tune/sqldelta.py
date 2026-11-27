import pathlib
import sys

COUNTERS = ('executions', 'buffer_gets', 'disk_reads', 'rows_processed', 'cpu_time', 'elapsed_time')


def read(path):
    rows = {}
    startup = None
    for line in pathlib.Path(path).read_text().splitlines():
        if line.startswith('startup '):
            startup = line.strip()
        if line.startswith('sql|'):
            _, sql_id, child, *values = line.split('|', 10)
            rows[(sql_id, int(child))] = (tuple(int(v) for v in values[:6]), values[7], values[6])
    if startup is None:
        raise ValueError(f'{path}: missing startup marker; incomplete snapshot')
    return startup, rows


def main():
    before_startup, before = read(sys.argv[1])
    after_startup, after = read(sys.argv[2])
    if before_startup != after_startup:
        raise ValueError('Oracle restarted between snapshots')
    print('Counters are cursor deltas; CPU and elapsed times are microseconds.')
    print('new: absent at start; reset/reloaded: unknown delta; missing: aged out at end.')
    print('sql_id child status ' + ' '.join(COUNTERS) + ' sql_text')
    for key in sorted(before.keys() | after.keys()):
        if key not in after:
            print(*key, 'missing', *(['?'] * 6), before[key][1])
            continue
        current, sql, incarnation = after[key]
        old = before.get(key, ((0,) * 6, ''))[0]
        delta = tuple(a - b for a, b in zip(current, old))
        status = 'new' if key not in before else 'ok'
        if key in before and incarnation != before[key][2]:
            print(*key, 'reloaded', *(['?'] * 6), sql)
        elif any(v < 0 for v in delta):
            print(*key, 'reset', *(['?'] * 6), sql)
        else:
            print(*key, status, *delta, sql)


if __name__ == '__main__':
    main()
