"""Control-plane tests. Fixtures must never reach the caller's Git data."""

import os
import subprocess

# A Git hook or a sandbox workaround can set these, and every fixture inherits them.
for name in subprocess.run(["git", "rev-parse", "--local-env-vars"], capture_output=True, text=True, check=True).stdout.split():
    os.environ.pop(name, None)
