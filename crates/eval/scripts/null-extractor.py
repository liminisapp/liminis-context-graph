#!/usr/bin/env python3
"""Eval-only (#863): an OpenAI-compatible `/v1/chat/completions` endpoint that extracts nothing.

`knowledge_search_passages` reads only Episodic nodes, so a probe that measures passage scores
needs the chunks ingested but no entities or edges. Pointing the service at this stub
(`--extractor-http http://127.0.0.1:<port>/v1/chat/completions`) ingests a corpus with no LLM, no
cassette and no cost. It is useless for anything that reads the graph (entities, relationships,
walk): those need a real extraction or a cassette that matches the current request shape.

usage: python3 -I crates/eval/scripts/null-extractor.py <port>   # binds 127.0.0.1 only
"""
import json
import sys
from http.server import BaseHTTPRequestHandler, HTTPServer

REPLY = json.dumps(
    {
        "id": "null-extractor",
        "object": "chat.completion",
        "choices": [
            {
                "index": 0,
                "finish_reason": "stop",
                "message": {"role": "assistant", "content": '{"entities": [], "edges": []}'},
            }
        ],
        "usage": {"prompt_tokens": 0, "completion_tokens": 0, "total_tokens": 0},
    }
).encode()


class Handler(BaseHTTPRequestHandler):
    def do_POST(self):
        self.rfile.read(int(self.headers.get("content-length", 0)))
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(REPLY)))
        self.end_headers()
        self.wfile.write(REPLY)

    def log_message(self, *args):
        pass


HTTPServer(("127.0.0.1", int(sys.argv[1])), Handler).serve_forever()
