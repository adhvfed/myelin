#!/usr/bin/env python3
"""
SPIKE: Luna-driven hosted-agent loop with per-turn token metering + a HARD spend cap.

Throwaway de-risking proof for Myelin's hosted-agent pillar. Not production code.
- Minimal agentic loop against gpt-5.6-luna via OpenAI chat/completions tool-calling.
- Read-only, path-jailed tools (list_dir / read_file / search / run-allowlisted).
- Meters every Luna call from the `usage` block using Luna's real pricing.
- HALTS before any paid call once running cost >= --cap-usd (runaway/abuse control).

Stdlib only (urllib/http.client). Key is read from $OPENAI_API_KEY and NEVER printed.
"""

import argparse
import json
import os
import subprocess
import sys
import urllib.request
import urllib.error

# ---- Luna real pricing (USD per token) -----------------------------------
# input $0.20/Mtok, output $1.20/Mtok, cache-hit $0.02/Mtok
PRICE_INPUT = 0.20 / 1_000_000
PRICE_OUTPUT = 1.20 / 1_000_000
PRICE_CACHE = 0.02 / 1_000_000

API_URL = "https://api.openai.com/v1/chat/completions"
MAX_READ_BYTES = 64 * 1024
MAX_TOOL_OUTPUT = 16 * 1024  # cap what we feed back to the model per tool result

RUN_ALLOWLIST = {"rg", "cat", "ls"}  # read-only commands only for the run() tool


# ---- path jail -----------------------------------------------------------
class JailError(Exception):
    pass


def jail(repo_root, path):
    """Resolve `path` (may be relative to repo_root) and ensure it stays inside repo_root."""
    if not path:
        path = "."
    if os.path.isabs(path):
        candidate = os.path.realpath(path)
    else:
        candidate = os.path.realpath(os.path.join(repo_root, path))
    root = os.path.realpath(repo_root)
    if candidate != root and not candidate.startswith(root + os.sep):
        raise JailError("path escapes repo jail: %r" % path)
    return candidate


# ---- tools ---------------------------------------------------------------
def tool_list_dir(repo_root, path="."):
    real = jail(repo_root, path)
    if not os.path.isdir(real):
        return "not a directory: %s" % path
    entries = []
    for name in sorted(os.listdir(real)):
        full = os.path.join(real, name)
        entries.append(name + ("/" if os.path.isdir(full) else ""))
    return "\n".join(entries) if entries else "(empty)"


def tool_read_file(repo_root, path):
    real = jail(repo_root, path)
    if not os.path.isfile(real):
        return "not a file: %s" % path
    with open(real, "rb") as f:
        data = f.read(MAX_READ_BYTES + 1)
    truncated = len(data) > MAX_READ_BYTES
    data = data[:MAX_READ_BYTES]
    text = data.decode("utf-8", errors="replace")
    if truncated:
        text += "\n...[truncated at %d bytes]" % MAX_READ_BYTES
    return text


def tool_search(repo_root, pattern, path="."):
    real = jail(repo_root, path)
    # ripgrep, bounded: line numbers, cap matches, cap total bytes fed back.
    try:
        out = subprocess.run(
            ["rg", "-n", "--max-count", "40", "--max-columns", "300", pattern, real],
            capture_output=True, text=True, timeout=30,
        )
    except FileNotFoundError:
        return "ripgrep (rg) not available"
    except subprocess.TimeoutExpired:
        return "search timed out"
    body = out.stdout or ("(no matches)" if out.returncode == 1 else out.stderr)
    # strip the jail prefix so the model sees repo-relative paths
    body = body.replace(os.path.realpath(repo_root) + os.sep, "")
    if len(body) > MAX_TOOL_OUTPUT:
        body = body[:MAX_TOOL_OUTPUT] + "\n...[truncated]"
    return body


def tool_run(repo_root, cmd):
    """Allowlisted read-only shell escape hatch — the untrusted-tool boundary in miniature."""
    parts = cmd.split()
    if not parts:
        return "empty command"
    prog = os.path.basename(parts[0])
    if prog not in RUN_ALLOWLIST:
        return "REFUSED: %r not in read-only allowlist %s" % (prog, sorted(RUN_ALLOWLIST))
    # Any path-like argument must resolve inside the jail.
    for a in parts[1:]:
        if a.startswith("-"):
            continue
        cand = a if os.path.isabs(a) else os.path.join(repo_root, a)
        if os.path.exists(cand) or os.path.isabs(a) or "/" in a:
            try:
                jail(repo_root, a)
            except JailError:
                return "REFUSED: argument escapes jail: %r" % a
    try:
        out = subprocess.run(parts, capture_output=True, text=True, timeout=30, cwd=repo_root)
    except Exception as e:  # noqa: BLE001
        return "run error: %s" % e
    body = (out.stdout + out.stderr)
    if len(body) > MAX_TOOL_OUTPUT:
        body = body[:MAX_TOOL_OUTPUT] + "\n...[truncated]"
    return body or "(no output)"


TOOLS_SCHEMA = [
    {"type": "function", "function": {
        "name": "list_dir",
        "description": "List the entries of a directory inside the target repo. Directories end with '/'.",
        "parameters": {"type": "object", "properties": {
            "path": {"type": "string", "description": "Repo-relative directory path. Default '.'."}},
            "required": []}}},
    {"type": "function", "function": {
        "name": "read_file",
        "description": "Read a text file inside the target repo (bounded to 64 KiB).",
        "parameters": {"type": "object", "properties": {
            "path": {"type": "string", "description": "Repo-relative file path."}},
            "required": ["path"]}}},
    {"type": "function", "function": {
        "name": "search",
        "description": "ripgrep for a regex/text pattern across the repo. Returns file:line: matches (bounded).",
        "parameters": {"type": "object", "properties": {
            "pattern": {"type": "string", "description": "The ripgrep pattern to search for."},
            "path": {"type": "string", "description": "Repo-relative subtree to search. Default '.'."}},
            "required": ["pattern"]}}},
    {"type": "function", "function": {
        "name": "run",
        "description": "Run a read-only shell command. ONLY rg/cat/ls are allowed; anything else is refused.",
        "parameters": {"type": "object", "properties": {
            "cmd": {"type": "string", "description": "The command line (e.g. 'rg -n foo src')."}},
            "required": ["cmd"]}}},
]

DISPATCH = {
    "list_dir": lambda root, a: tool_list_dir(root, a.get("path", ".")),
    "read_file": lambda root, a: tool_read_file(root, a["path"]),
    "search": lambda root, a: tool_search(root, a["pattern"], a.get("path", ".")),
    "run": lambda root, a: tool_run(root, a["cmd"]),
}


# ---- Luna call -----------------------------------------------------------
def luna_call(api_key, model, messages):
    payload = json.dumps({
        "model": model,
        "messages": messages,
        "tools": TOOLS_SCHEMA,
        "tool_choice": "auto",
        # Luna rejects function tools in /v1/chat/completions unless reasoning_effort is 'none'
        # (otherwise: use /v1/responses). 'none' keeps this a plain tool-calling loop.
        "reasoning_effort": "none",
    }).encode("utf-8")
    req = urllib.request.Request(API_URL, data=payload, method="POST")
    req.add_header("Authorization", "Bearer %s" % api_key)
    req.add_header("Content-Type", "application/json")
    try:
        with urllib.request.urlopen(req, timeout=120) as resp:
            return json.loads(resp.read().decode("utf-8"))
    except urllib.error.HTTPError as e:
        body = e.read().decode("utf-8", errors="replace")
        raise SystemExit("Luna HTTP %s: %s" % (e.code, body[:1000]))


def cost_of(usage):
    prompt = usage.get("prompt_tokens", 0)
    completion = usage.get("completion_tokens", 0)
    cached = (usage.get("prompt_tokens_details") or {}).get("cached_tokens", 0)
    non_cached_input = max(prompt - cached, 0)
    return (non_cached_input * PRICE_INPUT
            + cached * PRICE_CACHE
            + completion * PRICE_OUTPUT), prompt, completion, cached


# ---- agent loop ----------------------------------------------------------
def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--repo", required=True)
    ap.add_argument("--goal", required=True)
    ap.add_argument("--cap-usd", type=float, required=True)
    ap.add_argument("--model", default="gpt-5.6-luna")
    ap.add_argument("--max-turns", type=int, default=12)
    args = ap.parse_args()

    api_key = os.environ.get("OPENAI_API_KEY")
    if not api_key or "{{" in api_key:
        print("OPENAI_API_KEY NOT INJECTED", file=sys.stderr)
        return 3
    repo_root = os.path.realpath(args.repo)
    if not os.path.isdir(repo_root):
        print("repo not found: %s" % args.repo, file=sys.stderr)
        return 2

    print("== agent-spike ==")
    print("model=%s  repo=%s  cap=$%.4f" % (args.model, repo_root, args.cap_usd))
    print("goal: %s" % args.goal)
    print("-" * 60)

    system = (
        "You are a code-investigation agent operating INSIDE a target repository. "
        "You have read-only tools: list_dir, read_file, search (ripgrep), run (rg/cat/ls only). "
        "Use them to FIND answers in the actual source; do not rely on prior knowledge. "
        "When you are confident, give a final answer that names the exact file, function, "
        "and the concrete values you found. Be concise."
    )
    messages = [
        {"role": "system", "content": system},
        {"role": "user", "content": args.goal},
    ]

    total_cost = 0.0
    for turn in range(1, args.max_turns + 1):
        # HARD spend cap: check BEFORE making any paid call.
        if total_cost >= args.cap_usd:
            print("-" * 60)
            print("HALT: spend cap reached (running $%.6f >= cap $%.6f). "
                  "No further Luna call made." % (total_cost, args.cap_usd))
            return 42

        resp = luna_call(api_key, args.model, messages)
        usage = resp.get("usage", {}) or {}
        turn_cost, ptok, ctok, cached = cost_of(usage)
        total_cost += turn_cost
        print("[turn %d] tokens: prompt=%d (cached=%d) completion=%d | "
              "this turn=$%.6f | running total=$%.6f"
              % (turn, ptok, cached, ctok, turn_cost, total_cost))

        choice = resp["choices"][0]
        msg = choice["message"]
        # Echo the assistant message back into history (with any tool_calls).
        assistant_entry = {"role": "assistant", "content": msg.get("content")}
        tool_calls = msg.get("tool_calls")
        if tool_calls:
            assistant_entry["tool_calls"] = tool_calls
        messages.append(assistant_entry)

        if tool_calls:
            for tc in tool_calls:
                fn = tc["function"]["name"]
                try:
                    fargs = json.loads(tc["function"].get("arguments") or "{}")
                except json.JSONDecodeError:
                    fargs = {}
                try:
                    if fn in DISPATCH:
                        result = DISPATCH[fn](repo_root, fargs)
                    else:
                        result = "unknown tool: %s" % fn
                except JailError as e:
                    result = "JAIL REFUSED: %s" % e
                except KeyError as e:
                    result = "missing argument: %s" % e
                except Exception as e:  # noqa: BLE001
                    result = "tool error: %s" % e
                # compact one-line trace of what the agent did
                argstr = json.dumps(fargs, separators=(",", ":"))
                print("    -> %s(%s)  [%d bytes]" % (fn, argstr, len(str(result))))
                messages.append({
                    "role": "tool",
                    "tool_call_id": tc["id"],
                    "content": str(result),
                })
            continue  # let the model consume tool results next turn

        # No tool calls => final answer.
        print("-" * 60)
        print("FINAL ANSWER:")
        print(msg.get("content") or "(empty)")
        print("-" * 60)
        print("DONE in %d turn(s). total spend=$%.6f (cap=$%.4f)"
              % (turn, total_cost, args.cap_usd))
        return 0

    print("-" * 60)
    print("STOP: max turns (%d) reached. total spend=$%.6f" % (args.max_turns, total_cost))
    return 1


if __name__ == "__main__":
    sys.exit(main())
