import os
import subprocess

for name in subprocess.run(["git", "rev-parse", "--local-env-vars"], capture_output=True, text=True, check=True).stdout.split():
    os.environ.pop(name, None)
