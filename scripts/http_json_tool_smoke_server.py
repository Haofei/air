#!/usr/bin/env python3
import json
import os
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


class Handler(BaseHTTPRequestHandler):
    def do_POST(self):
        if self.path != "/search":
            self.send_error(404)
            return
        if self.headers.get("x-air-test") != "http-json-tool":
            self.send_error(400, "missing x-air-test header")
            return
        length = int(self.headers.get("content-length", "0"))
        payload = json.loads(self.rfile.read(length) or b"{}")
        response = {
            "query": payload.get("query", ""),
            "documents": [
                {
                    "id": "http-json",
                    "title": "HTTP JSON tool",
                    "content": f"received {payload.get('query', '')}",
                }
            ],
        }
        body = json.dumps(response).encode("utf-8")
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *_args):
        return


def main():
    port = int(os.environ.get("AIR_HTTP_TOOL_PORT", "0"))
    server = ThreadingHTTPServer(("127.0.0.1", port), Handler)
    print(server.server_port, flush=True)
    server.serve_forever()


if __name__ == "__main__":
    try:
        main()
    except KeyboardInterrupt:
        sys.exit(0)
