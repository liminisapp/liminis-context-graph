import json, socket, math
QP = "Represent this sentence for searching relevant passages: "
sock_p = None
_ECACHE_P = __import__("os").path.join(__import__("os").path.dirname(__import__("os").path.abspath(__file__)), "embed-cache.jsonl")
_ecache = None


def _load_ecache():
    global _ecache
    if _ecache is None:
        _ecache = {}
        try:
            for l in open(_ECACHE_P):
                try:
                    k, v = json.loads(l); _ecache[k] = v
                except Exception:
                    pass
        except FileNotFoundError:
            pass
    return _ecache


def _embed_batch(batch, sock):
    """One request with a timeout: the sidecar has twice left a connection hanging with no reply."""
    body = json.dumps({"input": batch, "model": "bge"}).encode()
    for attempt in range(5):
        try:
            s = socket.socket(socket.AF_UNIX); s.settimeout(60); s.connect(sock)
            s.sendall(b"POST /v1/embeddings HTTP/1.1\r\nHost: x\r\nContent-Type: application/json\r\n"
                      + f"Content-Length: {len(body)}\r\nConnection: close\r\n\r\n".encode() + body)
            buf = b""
            while True:
                c = s.recv(1 << 16)
                if not c: break
                buf += c
            s.close()
            head, _, payload = buf.partition(b"\r\n\r\n")
            if b"chunked" in head.lower():
                dec, rest = b"", payload
                while rest:
                    n, _, rest = rest.partition(b"\r\n"); n = int(n, 16)
                    if n == 0: break
                    dec, rest = dec + rest[:n], rest[n + 2:]
                payload = dec
            return [d["embedding"] for d in json.loads(payload)["data"]]
        except (socket.timeout, OSError, ValueError, KeyError) as e:
            import sys, time; print(f"embed retry {attempt + 1}: {type(e).__name__}", file=sys.stderr); time.sleep(2 << attempt)
    raise RuntimeError("embedder not answering")


def embed(texts, sock=None):
    """Cached on disk by exact text (embeddings are deterministic for one model and profile)."""
    import hashlib
    sock_ = sock or sock_p; C = _load_ecache()
    key = lambda x: hashlib.sha256(x.encode()).hexdigest()
    miss = list(dict.fromkeys(x for x in texts if key(x) not in C))
    with open(_ECACHE_P, "a") as f:
        for i in range(0, len(miss), 32):
            b = miss[i:i + 32]
            for x, v in zip(b, _embed_batch(b, sock_)):
                v = [round(y, 6) for y in v]; C[key(x)] = v; f.write(json.dumps([key(x), v]) + "\n")
    return [C[key(x)] for x in texts]


def cos(a, b):
    return sum(x * y for x, y in zip(a, b)) / (math.sqrt(sum(x * x for x in a)) * math.sqrt(sum(y * y for y in b)) + 1e-9)


