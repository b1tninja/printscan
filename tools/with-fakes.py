#!/usr/bin/env python3
"""Run a command with fakescan and fakeprint on loopback, and their base URLs in the environment.

    python tools/with-fakes.py --fakescan <dir> --fakeprint <dir> -- cargo test --workspace --all-features

<dir> is a checkout with the program built in <dir>/target/{release,debug} (or give the executable
with --fakescan-exe / --fakeprint-exe). fakescan runs --headless (an ephemeral port, no
WS-Discovery) and fakeprint on --fakeprint-port with discovery and JetDirect off. The command gets
PRINTSCAN_FAKESCAN_URL and PRINTSCAN_FAKEPRINT_URL; both fakes stop when it ends. See
docs/testing.md.
"""
import argparse
import json
import os
import socket
import subprocess
import sys
import tempfile
import time
from pathlib import Path


def find_exe(checkout: str, name: str) -> str:
    exe = name + (".exe" if os.name == "nt" else "")
    for build in ("release", "debug"):
        candidate = Path(checkout) / "target" / build / exe
        if candidate.is_file():
            return str(candidate)
    sys.exit(f"{name}: no {exe} under {checkout}/target/{{release,debug}}; build it or pass --{name}-exe")


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--fakescan", required=True, help="fakescan checkout (profiles/ and target/)")
    ap.add_argument("--fakescan-exe")
    ap.add_argument("--fakescan-profile", default="wia-lossless")
    ap.add_argument("--fakeprint", required=True, help="fakeprint checkout (profiles/ and target/)")
    ap.add_argument("--fakeprint-exe")
    ap.add_argument("--fakeprint-profile", default="generic-wsd")
    ap.add_argument("--fakeprint-port", type=int, default=3921)
    ap.add_argument("command", nargs=argparse.REMAINDER)
    args = ap.parse_args()
    command = args.command[1:] if args.command[:1] == ["--"] else args.command
    if not command:
        ap.error("give the command to run after --")

    scan_exe = args.fakescan_exe or find_exe(args.fakescan, "fakescan")
    print_exe = args.fakeprint_exe or find_exe(args.fakeprint, "fakeprint")
    inbox = tempfile.mkdtemp(prefix="fakeprint-inbox-")
    fakes = []
    try:
        scanner = subprocess.Popen(
            [scan_exe, "--headless", "--exit-on-stdin-close", "--profiles", str(Path(args.fakescan) / "profiles"),
             "--profile", args.fakescan_profile],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True,
        )
        fakes.append(scanner)
        line = scanner.stdout.readline()
        if not line.startswith("FAKESCAN_READY "):
            sys.exit(f"fakescan did not start: {line!r}")
        scan_url = json.loads(line.split(" ", 1)[1])["base_url"]

        printer = subprocess.Popen(
            [print_exe, "--listen", f"127.0.0.1:{args.fakeprint_port}", "--no-discovery", "--no-jetdirect",
             "--profiles", str(Path(args.fakeprint) / "profiles"), "--profile", args.fakeprint_profile,
             "--inbox", inbox, "--exit-on-stdin-close"],
            stdin=subprocess.PIPE, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        )
        fakes.append(printer)
        deadline = time.time() + 30
        while True:
            try:
                socket.create_connection(("127.0.0.1", args.fakeprint_port), timeout=0.5).close()
                break
            except OSError:
                if printer.poll() is not None or time.time() > deadline:
                    sys.exit("fakeprint did not start")
                time.sleep(0.1)
        print_url = f"http://127.0.0.1:{args.fakeprint_port}"

        print(f"fakescan  {scan_url}\nfakeprint {print_url}", flush=True)
        env = dict(os.environ, PRINTSCAN_FAKESCAN_URL=scan_url, PRINTSCAN_FAKEPRINT_URL=print_url)
        return subprocess.run(command, env=env).returncode
    finally:
        for fake in fakes:
            if fake.stdin:
                fake.stdin.close()
        for fake in fakes:
            try:
                fake.wait(timeout=20)
            except subprocess.TimeoutExpired:
                fake.kill()


if __name__ == "__main__":
    sys.exit(main())
