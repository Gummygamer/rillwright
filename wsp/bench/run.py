#!/usr/bin/env python3
"""Runs the wsp task set through `rillwright agent --json` and scores it.

Usage: wsp/bench/run.py --tag NAME [--model GGUF] [--split practice|heldout|all] [--seeds 1,2]
                        [--only id,id] [--loops N] [--selfcheck]

Each run gets a fresh fixture under target/wsp/bench/<tag>/<task>-s<seed>/ws. Commands run in
`full` mode inside the bwrap sandbox with no network and no web tool. Results append to
target/wsp/bench/<tag>/results.jsonl, and runs already recorded there are skipped, so an
interrupted run can be continued with the same tag. One process may use the GPU at a time.

A model that is not the pinned file needs its checksum in RILLWRIGHT_MODEL_SHA256; this script
computes and sets it when --model is given.
"""
import argparse
import hashlib
import json
import os
import shutil
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", ".."))
sys.path.insert(0, HERE)
from tasks import TASKS  # noqa: E402


def sha256(path):
    side = path + ".sha256"
    if os.path.exists(side) and os.path.getmtime(side) >= os.path.getmtime(path):
        return open(side).read().strip()
    h = hashlib.sha256()
    with open(path, "rb") as f:
        while chunk := f.read(8 << 20):
            h.update(chunk)
    try:
        with open(side, "w") as f:
            f.write(h.hexdigest() + "\n")
    except OSError:
        pass
    return h.hexdigest()


def run_task(task, seed, args, out_root, env):
    name = f"{task['id']}-s{seed}"
    base = os.path.join(out_root, name)
    shutil.rmtree(base, ignore_errors=True)
    ws = os.path.join(base, "ws")
    os.makedirs(ws)
    task["setup"](ws)
    cmd = [args.bin, "agent", task["prompt"], "--workspace", ws, "--permission", "full", "--sandbox", "bwrap",
           "--no-web", "--max-rounds", str(args.max_rounds), "--command-timeout", "60", "--max-seconds", "420",
           "--ctx", "16384", "--loops", str(args.loops), "--seed", str(seed), "--json"]
    if args.model:
        cmd += ["--model", args.model]
    if args.max_tokens:
        cmd += ["--max-tokens", str(args.max_tokens)]
    t0 = time.monotonic()
    try:
        p = subprocess.run(cmd, capture_output=True, text=True, timeout=600, env=env, start_new_session=True)
        code, out, err = p.returncode, p.stdout, p.stderr
    except subprocess.TimeoutExpired as e:
        out = e.stdout.decode(errors="replace") if isinstance(e.stdout, bytes) else (e.stdout or "")
        code, err = 124, "timed out"
    wall = time.monotonic() - t0
    with open(os.path.join(base, "stdout.jsonl"), "w") as f:
        f.write(out)
    with open(os.path.join(base, "stderr.txt"), "w") as f:
        f.write(err)
    events = []
    for line in out.splitlines():
        try:
            events.append(json.loads(line))
        except json.JSONDecodeError:
            pass
    fin = [e for e in events if e.get("event") == "finished"]
    answer = "".join(e.get("text", "") for e in events if e.get("event") == "content")
    results = [e.get("text", "") for e in events if e.get("event") == "tool_result"]
    try:
        ok, detail = task["check"](ws, answer)
    except Exception as e:  # a checker bug must show as a failure, not a crash
        ok, detail = False, f"checker error: {e!r}"
    f = fin[-1] if fin else {}
    return dict(id=task["id"], kind=task["kind"], split=task["split"], seed=seed, passed=bool(ok), detail=detail,
                exit=code, wall_s=round(wall, 1), reason=f.get("reason", "none"), rounds=f.get("rounds", 0),
                tool_calls=f.get("tool_calls", 0), generated_tokens=f.get("generated_tokens", 0),
                prompt_tokens=f.get("prompt_tokens", 0),
                malformed=sum(1 for e in events if e.get("event") == "malformed_call"),
                tool_errors=sum(1 for r in results if r.startswith("error:")),
                answer=answer.strip()[:300])


def summarize(rows):
    def rate(rs):
        return f"{sum(r['passed'] for r in rs)}/{len(rs)}" if rs else "-"
    print(f"\n{'task':<14}{'split':<10}{'pass':<7}{'rounds':>7}{'terr':>6}{'malf':>6}{'wall':>7}  reasons / first failure")
    for tid in dict.fromkeys(r["id"] for r in rows):
        rs = [r for r in rows if r["id"] == tid]
        bad = next((r for r in rs if not r["passed"]), None)
        reasons = ",".join(sorted({r["reason"] for r in rs}))
        print(f"{tid:<14}{rs[0]['split']:<10}{rate(rs):<7}{sum(r['rounds'] for r in rs) / len(rs):>7.1f}"
              f"{sum(r['tool_errors'] for r in rs):>6}{sum(r['malformed'] for r in rs):>6}"
              f"{sum(r['wall_s'] for r in rs) / len(rs):>7.0f}  {reasons}{' | ' + bad['detail'][:60] if bad else ''}")
    print()
    for split in ("practice", "heldout"):
        for kind in ("prog", "term"):
            rs = [r for r in rows if r["split"] == split and r["kind"] == kind]
            if rs:
                print(f"  {split:<9}{kind:<5} {rate(rs)}")
        rs = [r for r in rows if r["split"] == split]
        if rs:
            print(f"  {split:<9}all   {rate(rs)}  ({100 * sum(r['passed'] for r in rs) / len(rs):.1f}%)")
    print(f"  total          {rate(rows)}")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--tag", required=True)
    ap.add_argument("--model", default=None)
    ap.add_argument("--split", default="practice", choices=["practice", "heldout", "all"])
    ap.add_argument("--seeds", default="1,2")
    ap.add_argument("--only", default="")
    ap.add_argument("--loops", type=int, default=2)
    ap.add_argument("--max-rounds", type=int, default=16)
    ap.add_argument("--max-tokens", type=int, default=0, help="per-round generation limit; 0 leaves the harness default")
    ap.add_argument("--bin", default=os.path.join(ROOT, "target", "release", "rillwright"))
    ap.add_argument("--selfcheck", action="store_true", help="check every fixture builds and every checker fails on it untouched")
    ap.add_argument("--summary", action="store_true", help="print the summary of an existing tag and exit")
    args = ap.parse_args()

    if args.selfcheck:
        import tempfile
        for t in TASKS:
            with tempfile.TemporaryDirectory() as d:
                t["setup"](d)
                ok, detail = t["check"](d, "")
                print(f"{t['id']:<14}{t['kind']:<6}{t['split']:<10}{'UNEXPECTED PASS' if ok else 'fails untouched'}  {detail[:70]}")
        return

    out_root = os.path.join(ROOT, "target", "wsp", "bench", args.tag)
    os.makedirs(out_root, exist_ok=True)
    log_path = os.path.join(out_root, "results.jsonl")
    rows = [json.loads(l) for l in open(log_path)] if os.path.exists(log_path) else []
    if args.summary:
        summarize(rows)
        return
    done = {(r["id"], r["seed"]) for r in rows}
    env = dict(os.environ)
    for k in ("TAVILY_API_KEY", "BRAVE_SEARCH_API_KEY"):
        env.pop(k, None)
    if args.model:
        env["RILLWRIGHT_MODEL_SHA256"] = sha256(args.model)
    only = set(filter(None, args.only.split(",")))
    seeds = [int(s) for s in args.seeds.split(",")]
    with open(log_path, "a") as log:
        for task in TASKS:
            if (only and task["id"] not in only) or (args.split != "all" and task["split"] != args.split):
                continue
            for seed in seeds:
                if (task["id"], seed) in done:
                    continue
                r = run_task(task, seed, args, out_root, env)
                rows.append(r)
                log.write(json.dumps(r) + "\n")
                log.flush()
                print(f"{'PASS' if r['passed'] else 'FAIL'} {task['id']} s{seed} ({r['reason']}, {r['rounds']} rounds, {r['wall_s']}s): {r['detail'][:90]}", flush=True)
    summarize(rows)


if __name__ == "__main__":
    main()
