"""Headless `claude -p` on the Claude subscription (never an API key), with a disk cache.
Same flags as claude-subscription-bridge.py. Only counts are printed, never content."""
import concurrent.futures, hashlib, json, os, subprocess, threading, time

HERE = os.path.dirname(os.path.abspath(__file__))
CACHE_P = os.path.join(HERE, "llm-cache.jsonl")
ENV = {k: v for k, v in os.environ.items() if k not in ("ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_BASE_URL")}
ENV["MAX_THINKING_TOKENS"] = "0"
CWD = os.path.join(HERE, "llm-cwd"); os.makedirs(CWD, exist_ok=True)
_lock = threading.Lock()
_cache = {}
if os.path.exists(CACHE_P):
    for l in open(CACHE_P):
        try:
            x = json.loads(l); _cache[x["k"]] = x["v"]
        except Exception:
            pass


def call(model, system, prompt, schema):
    k = hashlib.sha256(json.dumps([model, system, prompt, schema]).encode()).hexdigest()
    if k in _cache:
        return _cache[k]
    cmd = ["claude", "-p", "--model", model, "--output-format", "json", "--no-session-persistence",
           "--tools", "", "--strict-mcp-config", "--setting-sources", "",
           "--settings", '{"alwaysThinkingEnabled":false}', "--system-prompt", system,
           "--json-schema", json.dumps(schema)]
    last = None
    for attempt in range(4):
        p = subprocess.run(cmd, input=prompt, capture_output=True, text=True, env=ENV, cwd=CWD, timeout=600)
        try:
            d = json.loads(p.stdout)
        except json.JSONDecodeError:
            last = f"exit {p.returncode}: unparseable"; time.sleep(2 << attempt); continue
        if d.get("is_error") or d.get("structured_output") is None:
            last = f"error {d.get('subtype')}"; time.sleep(2 << attempt); continue
        v = d["structured_output"]
        with _lock:
            _cache[k] = v
            with open(CACHE_P, "a") as f:
                f.write(json.dumps({"k": k, "v": v}) + "\n")
        return v
    raise RuntimeError(last)


def map_calls(jobs, workers=6):
    """jobs: list of (model, system, prompt, schema). Returns results in order (None on failure)."""
    # Identical prompts are sent once, so the same context always gets the same answer.
    uniq = list(dict.fromkeys(json.dumps(j) for j in jobs))
    res = {}; done = [0, 0]
    with concurrent.futures.ThreadPoolExecutor(workers) as ex:
        futs = {ex.submit(call, *json.loads(u)): u for u in uniq}
        for f in concurrent.futures.as_completed(futs):
            try:
                res[futs[f]] = f.result(); done[0] += 1
            except Exception:
                done[1] += 1
    print(f"llm: {done[0]} ok, {done[1]} failed of {len(uniq)} unique ({len(jobs)} jobs)")
    return [res.get(json.dumps(j)) for j in jobs]
