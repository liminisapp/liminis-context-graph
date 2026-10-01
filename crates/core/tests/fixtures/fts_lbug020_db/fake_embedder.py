#!/usr/bin/env python3
"""Deterministic fake OpenAI-compatible /v1/embeddings server (dim 768) for building the fixture."""
import hashlib, json, struct
from http.server import BaseHTTPRequestHandler, HTTPServer

DIM = 768

def vec(text):
    out, i = [], 0
    while len(out) < DIM:
        h = hashlib.sha256(f"{i}:{text}".encode()).digest()
        out += [(b / 255.0) - 0.5 for b in h]
        i += 1
    return out[:DIM]

class H(BaseHTTPRequestHandler):
    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        inp = body["input"]
        if isinstance(inp, str):
            inp = [inp]
        resp = {"object": "list", "model": body.get("model", "fake"),
                "data": [{"object": "embedding", "index": i, "embedding": vec(t)} for i, t in enumerate(inp)]}
        data = json.dumps(resp).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)
    def log_message(self, *a): pass

HTTPServer(("127.0.0.1", 8765), H).serve_forever()
