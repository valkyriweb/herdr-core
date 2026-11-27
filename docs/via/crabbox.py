#!/usr/bin/env python3
"""Register a brokered Crabbox serve lease as an hc machine (macOS or Linux)."""
import json
import os
import re
import shlex
import shutil
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path


def run(argv, *, cwd=None):
    return subprocess.run(argv, cwd=cwd, stdin=subprocess.DEVNULL, capture_output=True, text=True)


@dataclass(frozen=True)
class Route:
    lease: str
    repo: str
    hc: str
    crabbox: str

    @property
    def name(self):
        return self.lease.replace("_", "-", 1)

    @property
    def via(self):
        argv = [sys.executable, str(Path(__file__).resolve()), "_connect", self.lease, self.repo, self.hc, self.crabbox]
        return "exec " + shlex.join(argv)

    def broker(self, command, *args):
        return run([self.crabbox, command, "--provider", "serve", "--id", self.lease, *args], cwd=self.repo)


def ended(route):
    status = route.broker("status", "--json")
    if status.returncode:
        return any(
            line == f'coordinator GET /v1/leases/{route.lease}: http 404: {{"error":"not_found"}}'
            for line in status.stderr.splitlines()
        )
    lease = json.loads(status.stdout)
    return lease["id"] == route.lease and lease["provider"] == "serve" and lease["state"] in {"expired", "released"}


def forget(route):
    listed = run([route.hc, "machine", "ls", "--json"])
    if listed.returncode:
        raise RuntimeError(listed.stderr.strip())
    for machine in json.loads(listed.stdout)["machines"]:
        if machine["via"] and machine["target"] == route.via:
            # The core can cancel this transport while handling machine.rm.
            removed = run([route.hc, "machine", "rm", machine["name"]])
            if removed.returncode:
                raise RuntimeError(removed.stderr.strip())
            return


def connect(route, line):
    resolved = route.broker("ssh")
    if resolved.returncode:
        if ended(route):
            forget(route)
        sys.stderr.write(resolved.stderr)
        return resolved.returncode
    argv = shlex.split(resolved.stdout)
    if len(resolved.stdout.splitlines()) != 1 or len(argv) < 2 or argv[0] != "ssh":
        raise RuntimeError("expected one SSH command from crabbox ssh")
    # Use the broker's pinned route without giving its resolver the framed input.
    os.execvp("ssh", [*argv[:-1], "-T", argv[-1], line])


def main(argv):
    if len(argv) < 2 or not re.fullmatch(r"cbx_[a-f0-9]{12}", argv[1]):
        raise RuntimeError("usage: crabbox.py add <canonical cbx_… lease ID>")
    if len(argv) == 6 and argv[0] == "_connect":
        return connect(Route(*argv[1:5]), argv[5])
    if len(argv) != 2 or argv[0] != "add":
        raise RuntimeError("usage: crabbox.py add <canonical cbx_… lease ID>")
    hc, crabbox = shutil.which("hc"), shutil.which("crabbox")
    if not hc or not crabbox:
        raise RuntimeError("put hc and crabbox on PATH before registering a lease")
    route = Route(argv[1], os.getcwd(), os.path.abspath(hc), os.path.abspath(crabbox))
    added = run([route.hc, "machine", "add", "--install", "--name", route.name, "--via", route.via, "--json"])
    sys.stderr.write(added.stderr)
    if added.returncode == 0:
        print(json.loads(added.stdout)["name"])
    elif added.stdout:
        sys.stderr.write(added.stdout)
    return added.returncode


if __name__ == "__main__":
    try:
        sys.exit(main(sys.argv[1:]))
    except (OSError, ValueError, KeyError, RuntimeError) as error:
        sys.exit(f"hc Crabbox machine: {error}")
