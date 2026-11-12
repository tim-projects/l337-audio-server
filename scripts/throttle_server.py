#!/usr/bin/env python3
"""Throttled HTTP server to reproduce MKV demuxer wedge without YouTube rate limits.

Serves a WebM/Opus file at a configurable byte rate to simulate a slow connection,
triggering the Symphonia MKV reader's 'past master element' wedge when an element
crosses the probe-time size boundary in a partially-downloaded file.

Usage:
    python3 throttle_server.py --file /path/to/test_251.webm --rate 32768 --port 18080

Endpoints:
    GET /<any>    - serve the file at --rate bytes/sec
    GET /size     - return file size as text (for readiness polling)
"""

import http.server
import socketserver
import time
import argparse
import os
import threading


class ThrottledHandler(http.server.BaseHTTPRequestHandler):
    file_path = None
    rate_bytes_per_sec = None  # None = unlimited

    def log_message(self, format, *args):
        # Silence the default log spam
        pass

    def do_GET(self):
        if self.path == "/size":
            self.send_response(200)
            self.send_header("Content-Type", "text/plain")
            self.end_headers()
            self.wfile.write(str(os.path.getsize(self.file_path)).encode())
        else:
            self._serve_file(unlimited=False)

    def _serve_file(self, unlimited=False):
        file_size = os.path.getsize(self.file_path)
        self.send_response(200)
        self.send_header("Content-Type", "audio/webm")
        self.send_header("Content-Length", str(file_size))
        self.end_headers()

        chunk_size = 4096
        with open(self.file_path, "rb") as f:
            while True:
                chunk = f.read(chunk_size)
                if not chunk:
                    break
                self.wfile.write(chunk)
                if not unlimited and self.rate_bytes_per_sec:
                    # Sleep to maintain the target rate
                    time.sleep(chunk_size / self.rate_bytes_per_sec)


def run_server(file_path, rate, port):
    ThrottledHandler.file_path = file_path
    ThrottledHandler.rate_bytes_per_sec = rate
    socketserver.TCPServer.allow_reuse_address = True
    with socketserver.TCPServer(("127.0.0.1", port), ThrottledHandler) as httpd:
        print(f"Serving {file_path} at {rate} bytes/sec on port {port}")
        httpd.serve_forever()


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description="Throttled HTTP server for MKV wedge testing")
    parser.add_argument("--file", required=True, help="Path to WebM/Opus file to serve")
    parser.add_argument("--rate", type=int, default=32768, help="Bytes per second (default: 32768 ≈ 32 KB/s)")
    parser.add_argument("--port", type=int, default=18080, help="Port to listen on")
    args = parser.parse_args()

    if not os.path.exists(args.file):
        print(f"Error: file not found: {args.file}")
        exit(1)

    run_server(args.file, args.rate, args.port)