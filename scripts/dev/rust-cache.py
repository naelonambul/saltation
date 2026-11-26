import argparse
import fcntl
import hashlib
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import sys
import tempfile
import time


class CacheError(Exception):
    pass


def digest_parts(parts):
    h = hashlib.sha256()
    for value in parts:
        data = value if isinstance(value, bytes) else str(value).encode()
        h.update(len(data).to_bytes(8, 'big'))
        h.update(data)
    return h.hexdigest()


def arguments():
    try:
        split = sys.argv.index('--')
    except ValueError:
        raise CacheError('expected -- cargo build/test/clippy/run')
    parser = argparse.ArgumentParser(allow_abbrev=False)
    parser.add_argument('--bank', required=True, type=Path)
    parser.add_argument('--contracts', required=True, type=Path)
    parser.add_argument('--profile', required=True)
    opts = parser.parse_args(sys.argv[1:split])
    cmd = sys.argv[split + 1:]
    if len(cmd) < 2 or cmd[0] != 'cargo' or cmd[1] not in ('build', 'test', 'clippy', 'run'):
        raise CacheError('unsupported Cargo command')
    cargo_args = cmd[2:]
    cargo_args = cargo_args[:cargo_args.index('--')] if '--' in cargo_args else cargo_args
    profiles, targets = [], []
    release = False
    index = 0
    while index < len(cargo_args):
        arg = cargo_args[index]
        if any(arg.startswith(p) for p in ('--manifest', '--target-dir', '--config', '--artifact-dir', '--out-dir')):
            raise CacheError('unsupported Cargo path/config override')
        if arg in ('--release', '-r'):
            release = True
        elif arg == '--profile' or arg.startswith('--profile='):
            if arg == '--profile':
                index += 1
                if index == len(cargo_args):
                    raise CacheError('missing Cargo profile')
                profiles.append(cargo_args[index])
            else:
                profiles.append(arg.partition('=')[2])
        elif arg == '--target' or arg.startswith('--target='):
            if arg == '--target':
                index += 1
                if index == len(cargo_args):
                    raise CacheError('missing Cargo target')
                targets.append(cargo_args[index])
            else:
                targets.append(arg.partition('=')[2])
        index += 1
    if len(profiles) > 1 or len(targets) > 1 or (release and profiles):
        raise CacheError('ambiguous Cargo profile/target')
    actual = profiles[0] if profiles else ('release' if release else 'dev')
    if not re.fullmatch(r'[A-Za-z0-9_][A-Za-z0-9_-]*', actual):
        raise CacheError('invalid Cargo profile')
    effective = 'debug' if actual in ('dev', 'test') else actual
    if opts.profile == 'custom':
        if not profiles or effective in ('debug', 'release'):
            raise CacheError('custom requires an explicit custom Cargo --profile')
    elif opts.profile != effective:
        raise CacheError('wrapper and Cargo profiles do not match')
    if targets and (not targets[0] or targets[0].startswith('-')):
        raise CacheError('invalid Cargo target')
    bank, contracts = opts.bank.resolve(), opts.contracts.resolve()
    if not (bank / 'Cargo.toml').is_file() or not contracts.is_dir():
        raise CacheError('bank manifest/contracts directory missing')
    raw_target = Path(os.environ.get('CARGO_TARGET_DIR') or 'target')
    target_dir = (bank / raw_target).resolve()
    return bank, contracts, target_dir, effective, actual, targets, cmd


def config_paths(bank):
    paths = set()
    for parent in (bank, *bank.parents):
        paths.update((parent / '.cargo' / 'config', parent / '.cargo' / 'config.toml'))
    home = Path(os.environ.get('CARGO_HOME', str(Path.home() / '.cargo')))
    if not home.is_absolute():
        home = bank / home
    paths.update((home / 'config', home / 'config.toml'))
    return sorted(paths)


def file_parts(path):
    if path.is_symlink():
        raise CacheError('symlink input cannot be fingerprinted safely')
    return [str(path), path.read_bytes() if path.is_file() else b'<missing>']


def tree_parts(root):
    if root.is_symlink():
        raise CacheError('symlink input cannot be fingerprinted safely')
    parts = [str(root)]
    if not root.exists():
        return parts + [b'<missing>']
    for directory, dirs, files in os.walk(root, followlinks=False):
        dirs[:] = sorted(d for d in dirs if d not in ('.git', 'target', '__pycache__') and not d.startswith('.env'))
        for name in dirs:
            if (Path(directory) / name).is_symlink():
                raise CacheError('symlink input cannot be fingerprinted safely')
        for name in sorted(files):
            if name.startswith('.env') or name.lower() in ('credentials', 'credentials.toml'):
                continue
            parts.extend(file_parts(Path(directory) / name))
    return parts


def compile_env():
    exact = {'RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS', 'RUSTDOCFLAGS',
             'CARGO_ENCODED_RUSTDOCFLAGS', 'RUSTC', 'RUSTDOC', 'RUSTUP_TOOLCHAIN',
             'RUSTC_WRAPPER', 'RUSTC_WORKSPACE_WRAPPER', 'CC', 'CXX', 'AR',
             'CFLAGS', 'CXXFLAGS', 'CPPFLAGS', 'LDFLAGS', 'HOST', 'TARGET',
             'PATH', 'CARGO_HOME', 'RUSTUP_HOME', 'SOURCE_DATE_EPOCH'}
    prefixes = ('CARGO_PROFILE_', 'CARGO_BUILD_', 'CARGO_TARGET_', 'CARGO_CFG_',
                'CC_', 'CXX_', 'AR_', 'CFLAGS_', 'CXXFLAGS_', 'PKG_CONFIG_')
    excluded = {'CARGO_BUILD_JOBS'}
    return digest_parts([item for key in sorted(os.environ)
                         if key not in excluded
                         and (key in exact or key.startswith(prefixes))
                         and not any(word in key.upper() for word in ('TOKEN', 'PASSWORD', 'SECRET', 'CREDENTIAL'))
                         for item in (key, os.environ[key])])


class Children:
    def __init__(self):
        self.child = None
        self.interrupted = 0

    def signal(self, number, frame):
        self.interrupted = self.interrupted or number
        if self.child is not None:
            try:
                os.killpg(self.child.pid, number)
            except ProcessLookupError:
                pass

    def check(self):
        if self.interrupted:
            raise CacheError('interrupted')

    def run(self, command, bank, env, capture=False):
        self.check()
        self.child = subprocess.Popen(command, cwd=bank, env=env,
                                      start_new_session=True,
                                      stdout=subprocess.PIPE if capture else None,
                                      stderr=subprocess.DEVNULL if capture else None)
        if self.interrupted:
            self.signal(self.interrupted, None)
        child = self.child
        try:
            output, _ = child.communicate()
            return child.returncode, output or b''
        finally:
            self.child = None


def _cargo_fingerprint_args(cmd):
    args = cmd[:cmd.index('--')] if len(cmd) > 1 and cmd[1] in ('test', 'run') and '--' in cmd else list(cmd)
    if len(args) < 2 or args[1] != 'test':
        return args
    filtered = []
    index = 0
    while index < len(args):
        arg = args[index]
        if arg == '--test':
            index += 2
            continue
        if arg.startswith('--test=') or arg in ('--tests', '--no-run', '--lib', '--bins'):
            index += 1
            continue
        filtered.append(arg)
        index += 1
    return filtered


def fingerprint(bank, contracts, config, cmd, children, env):
    parts = [str(bank), str(contracts), compile_env()]
    parts.extend(file_parts(Path(__file__).resolve()))
    parts.extend(tree_parts(bank))
    parts.extend(tree_parts(contracts))
    for path in config:
        parts.extend(file_parts(path))
    for command in (['rustc', '-vV'], ['cargo', '-V']):
        status, output = children.run(command, bank, env, capture=True)
        if status:
            raise CacheError('compiler identity unavailable')
        parts.extend((command[0], output))
    parts.extend(_cargo_fingerprint_args(cmd))
    return digest_parts(parts)


def atomic_marker(path, record):
    fd, temporary = tempfile.mkstemp(prefix='.marker-', dir=path.parent)
    try:
        with os.fdopen(fd, 'w') as stream:
            json.dump(record, stream, sort_keys=True)
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temporary, path)
        directory = os.open(str(path.parent), os.O_RDONLY)
        try:
            os.fsync(directory)
        finally:
            os.close(directory)
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)


def trusted(path, source):
    try:
        if path.stat().st_size > 4096:
            return False
        record = json.loads(path.read_text())
        return record == {'version': 1, 'trusted': True, 'source': source}
    except (OSError, ValueError, TypeError):
        return False


def main(children):
    bank, contracts, target, profile, actual, targets, command = arguments()
    env = dict(os.environ, CARGO_TARGET_DIR=str(target))
    state = target / '.saltation-rust-cache'
    state.mkdir(parents=True, exist_ok=True)
    with (state / 'lock').open('a+b') as lock:
        while True:
            children.check()
            try:
                fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
                break
            except BlockingIOError:
                time.sleep(.05)
        config = config_paths(bank)
        subcmd_bucket = 'clippy' if len(command) > 1 and command[1] == 'clippy' else 'build'
        selector = digest_parts([*targets, os.environ.get('CARGO_BUILD_TARGET', ''),
                                 *[part for path in config for part in file_parts(path)]])
        profile_scope = f'{profile}-{subcmd_bucket}'
        key = digest_parts([profile_scope, selector])
        marker = state / (digest_parts([profile_scope]) + '-' + key + '.json')
        source = fingerprint(bank, contracts, config, command, children, env)
        invalidated = not trusted(marker, source)
        for sibling in state.glob(digest_parts([profile_scope]) + '-*.json'):
            if sibling != marker:
                atomic_marker(sibling, {'version': 1, 'trusted': False})
        atomic_marker(marker, {'version': 1, 'trusted': False})
        print('rust-cache: source={} profile={} invalidated={}'.format(
            source, profile, 'yes' if invalidated else 'no'), file=sys.stderr)
        children.check()
        if invalidated:
            clean = ['cargo', 'clean', '--package', 'bank', '--locked']
            if profile == 'release':
                clean.append('--release')
            elif profile != 'debug':
                clean.extend(['--profile', actual])
            if targets:
                clean.extend(['--target', targets[0]])
            status, _ = children.run(clean, bank, env)
            if status or children.interrupted:
                return status or 128 + children.interrupted
        status, _ = children.run(command, bank, env)
        if status or children.interrupted:
            return status or 128 + children.interrupted
        after = fingerprint(bank, contracts, config_paths(bank), command, children, env)
        children.check()
        if source != after:
            raise CacheError('inputs changed during Cargo execution; cache remains untrusted')
        atomic_marker(marker, {'version': 1, 'trusted': True, 'source': source})
        return 0


if __name__ == '__main__':
    children = Children()
    for number in (signal.SIGINT, signal.SIGTERM):
        signal.signal(number, children.signal)
    try:
        result = main(children)
    except (CacheError, OSError):
        print('rust-cache: interrupted' if children.interrupted else
              'rust-cache: rejected arguments or unavailable/changed inputs; cache not trusted', file=sys.stderr)
        result = 128 + children.interrupted if children.interrupted else 2
    if result < 0:
        result = 128 - result
    sys.exit(result)
