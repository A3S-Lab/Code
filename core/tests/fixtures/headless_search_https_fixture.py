#!/usr/bin/env python3
"""Controlled HTTPS page used by the hermetic headless-search qualification."""

from __future__ import annotations

import argparse
import json
import ssl
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import urlparse


FIXTURE_TITLE = "A3S Controlled CDP Fixture"
FIXTURE_URL = "https://docs.a3s.dev/controlled-cdp-fixture"


class FixtureHandler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    log_path: Path

    def do_GET(self) -> None:  # noqa: N802 - BaseHTTPRequestHandler contract
        parsed = urlparse(self.path)
        self._record(parsed.path)
        if parsed.path == "/health":
            self._respond(200, "text/plain; charset=utf-8", b"ok\n")
            return
        if parsed.path != "/search":
            self._respond(404, "text/plain; charset=utf-8", b"not found\n")
            return

        body = f"""<!doctype html>
<html lang="en">
  <head><meta charset="utf-8"><title>Controlled Search Fixture</title></head>
  <body>
    <main id="search">
      <div class="g">
        <a href="{FIXTURE_URL}"><h3>{FIXTURE_TITLE}</h3></a>
        <div class="VwiC3b">A deterministic local page rendered through the production CDP browser path.</div>
      </div>
    </main>
  </body>
</html>
""".encode("utf-8")
        self._respond(200, "text/html; charset=utf-8", body)

    def log_message(self, _format: str, *_args: object) -> None:
        return

    def _record(self, path: str) -> None:
        record = {
            "client": self.client_address[0],
            "host": self.headers.get("host"),
            "path": path,
        }
        with self.log_path.open("a", encoding="utf-8") as stream:
            stream.write(json.dumps(record, sort_keys=True) + "\n")

    def _respond(self, status: int, content_type: str, body: bytes) -> None:
        self.send_response(status)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Connection", "close")
        self.end_headers()
        self.wfile.write(body)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--cert", required=True, type=Path)
    parser.add_argument("--key", required=True, type=Path)
    parser.add_argument("--log", required=True, type=Path)
    parser.add_argument("--port", default=443, type=int)
    return parser.parse_args()


def main() -> None:
    args = parse_args()
    FixtureHandler.log_path = args.log
    server = ThreadingHTTPServer(("127.0.0.1", args.port), FixtureHandler)
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    context.load_cert_chain(args.cert, args.key)
    server.socket = context.wrap_socket(server.socket, server_side=True)
    server.serve_forever()


if __name__ == "__main__":
    main()
