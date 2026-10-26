#!/usr/bin/env python3
"""Serve the browser page, and take the measurement it posts back.

The page runs the engine's kernel in a browser and posts its result here.
This server does two things:

- serves `target/web`, which is where `just web-build` puts the module and
  the page;
- accepts a `POST /report` and writes the JSON the page posted to
  `target/web/browser-report.json`.

The report is written to `target/`, not to `bench-results/`, on purpose.
`bench-results/` holds **published** measurements, and publishing one is a
deliberate act -- `just web-report` copies it in. A reader who runs the page
to see it work should not silently overwrite a committed number.

Usage:

    python3 scripts/web-server.py [port]
"""

from __future__ import annotations

import http.server
import json
import os
import socketserver
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SERVE = ROOT / "target" / "web"
REPORT = SERVE / "browser-report.json"

# The WebAssembly MIME type. `WebAssembly.instantiateStreaming` refuses a
# module served as anything else, and the page should not have to care.
http.server.SimpleHTTPRequestHandler.extensions_map[".wasm"] = "application/wasm"


class Handler(http.server.SimpleHTTPRequestHandler):
    """Serves the page, and takes the measurement it posts back."""

    def __init__(self, *args: object, **kwargs: object) -> None:
        super().__init__(*args, directory=str(SERVE), **kwargs)

    def do_POST(self) -> None:  # noqa: N802 -- the name is http.server's
        """Accepts the page's measurement."""
        if self.path != "/report":
            self.send_error(404, "the only POST here is /report")
            return
        length = int(self.headers.get("Content-Length", "0"))
        body = self.rfile.read(length)
        try:
            report = json.loads(body.decode("utf-8"))
        except json.JSONDecodeError as error:
            self.send_error(400, f"the report is not JSON: {error}")
            return
        # The host's load is a condition of a measurement taken on it, and
        # this machine is not isolated. The page cannot see the host's load;
        # the server can.
        report["host_load_average"] = list(os.getloadavg())
        report["host_cpu_count"] = os.cpu_count()
        # The commit goes here rather than into the module, because cargo
        # caches a build script: a commit baked into the module can be a
        # commit the module was not built from.
        report["commit"] = current_commit()
        REPORT.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
        self.send_response(204)
        self.end_headers()

    def do_GET(self) -> None:  # noqa: N802 -- the name is http.server's
        """Serves the commit, alongside the page and the module."""
        if self.path != "/commit":
            super().do_GET()
            return
        body = current_commit().encode("utf-8")
        self.send_response(200)
        self.send_header("Content-Type", "text/plain; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, format: str, *args: object) -> None:
        sys.stderr.write("%s\n" % (format % args))


def current_commit() -> str:
    """The commit the tree is at, with `-dirty` when the tree is not clean.

    This is the commit the measurement was taken under, read at the moment
    the measurement is taken rather than at the moment the module was built.

    Only **tracked** changes make the tree dirty: the raw reports are written
    next to the code, and an untracked report is not a modified source file.
    """
    try:
        commit = subprocess.run(
            ["git", "rev-parse", "--short", "HEAD"],
            cwd=ROOT,
            capture_output=True,
            check=True,
            text=True,
        ).stdout.strip()
    except (OSError, subprocess.CalledProcessError):
        return "unknown"
    if not commit:
        return "unknown"
    try:
        status = subprocess.run(
            ["git", "status", "--porcelain", "--untracked-files=no"],
            cwd=ROOT,
            capture_output=True,
            check=True,
            text=True,
        ).stdout.strip()
    except (OSError, subprocess.CalledProcessError):
        return commit
    return f"{commit}-dirty" if status else commit


class Server(socketserver.ThreadingTCPServer):
    """A server that can be asked to stop."""

    allow_reuse_address = True
    daemon_threads = True


def main() -> int:
    """Serves the page until interrupted."""
    if not SERVE.is_dir():
        print(
            f"{SERVE} does not exist. Run `just web-build` first.",
            file=sys.stderr,
        )
        return 1
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 8080
    with Server(("127.0.0.1", port), Handler) as httpd:
        print(f"serving {SERVE} on http://localhost:{port}", flush=True)
        print("press ctrl-c to stop", flush=True)
        try:
            httpd.serve_forever()
        except KeyboardInterrupt:
            print("", flush=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
