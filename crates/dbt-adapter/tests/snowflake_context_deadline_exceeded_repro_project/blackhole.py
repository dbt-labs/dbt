#!/usr/bin/env python3
"""TCP black-hole server for Snowflake login timeout repro.

Accepts connections, drains client writes, and never sends HTTP headers back.
This mimics a slow or unresponsive Snowflake auth endpoint and triggers errors
like:

  context deadline exceeded (Client.Timeout exceeded while awaiting headers)

Matches the listener in `snowflake_context_deadline_exceeded.rs`.
"""

from __future__ import annotations

import argparse
import errno
import os
import re
import signal
import socket
import subprocess
import sys
import threading
import time

DEFAULT_HOST = "127.0.0.1"
DEFAULT_PORT = 9999


def handle(conn: socket.socket, addr: tuple[str, int]) -> None:
    print(f"accepted {addr}", flush=True)
    try:
        while conn.recv(4096):
            pass
    except OSError:
        pass
    finally:
        conn.close()
        print(f"closed {addr}", flush=True)


def _cmdline(pid: int) -> str:
    try:
        return (
            open(f"/proc/{pid}/cmdline", "rb")
            .read()
            .replace(b"\0", b" ")
            .decode(errors="replace")
            .strip()
            or "<unknown>"
        )
    except OSError:
        return "<unknown>"


def _listener_pids(port: int) -> list[int]:
    result = subprocess.run(
        ["ss", "-tlnp", f"sport = :{port}"],
        capture_output=True,
        text=True,
        check=False,
    )
    return sorted({int(m.group(1)) for m in re.finditer(r"pid=(\d+)", result.stdout)})


def _resolve_port_conflict(host: str, port: int, assume_yes: bool) -> None:
    pids = _listener_pids(port)
    print(f"Port {host}:{port} is already in use:", flush=True)
    if not pids:
        sys.exit("Could not find the listening process.")

    for pid in pids:
        cmd = _cmdline(pid)
        try:
            ppid = int(next(l.split()[1] for l in open(f"/proc/{pid}/status") if l.startswith("PPid:")))
            caller = _cmdline(ppid)
        except (OSError, StopIteration):
            ppid, caller = "?", "<unknown>"
        print(f"  PID {pid}: {cmd}", flush=True)
        print(f"    caller (PPID {ppid}): {caller}", flush=True)

    if not assume_yes:
        try:
            ok = input("Kill these process(es) and continue? [y/N] ").strip().lower() in ("y", "yes")
        except (EOFError, KeyboardInterrupt):
            sys.exit("\nAborted.")
        if not ok:
            sys.exit("Aborted.")

    for pid in pids:
        os.kill(pid, signal.SIGTERM)
    time.sleep(0.2)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--host", default=DEFAULT_HOST)
    parser.add_argument("--port", type=int, default=DEFAULT_PORT)
    parser.add_argument("--yes", action="store_true", help="kill without prompting")
    args = parser.parse_args()

    server = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    server.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    try:
        server.bind((args.host, args.port))
    except OSError as exc:
        if exc.errno != errno.EADDRINUSE:
            raise
        _resolve_port_conflict(args.host, args.port, args.yes)
        server.bind((args.host, args.port))

    with server:
        server.listen()
        print(f"blackhole listening on {args.host}:{args.port}", flush=True)
        print("Ctrl+C to stop", flush=True)
        while True:
            try:
                conn, addr = server.accept()
            except KeyboardInterrupt:
                print("\nstopped", flush=True)
                return 0
            threading.Thread(target=handle, args=(conn, addr), daemon=True).start()


if __name__ == "__main__":
    sys.exit(main())
