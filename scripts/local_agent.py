#!/usr/bin/env python3
"""
CLI helper to query the local Qwen agent (via Ollama on gateway.docker.internal:11434).
Used for boilerplate, codecs, SQL queries, and unit tests as specified in AGENTS.md.
"""

import sys
import json
import argparse
import urllib.request
import urllib.error

DEFAULT_ENDPOINT = "http://gateway.docker.internal:11434/api/chat"
DEFAULT_MODEL = "qwen2.5-coder:14b"

def query_qwen(prompt: str, model: str = DEFAULT_MODEL, endpoint: str = DEFAULT_ENDPOINT, system: str = None) -> str:
    messages = []
    if system:
        messages.append({"role": "system", "content": system})
    messages.append({"role": "user", "content": prompt})

    payload = {
        "model": model,
        "messages": messages,
        "stream": False,
        "options": {
            "temperature": 0.2,
        }
    }

    req = urllib.request.Request(
        endpoint,
        data=json.dumps(payload).encode("utf-8"),
        headers={"Content-Type": "application/json"},
        method="POST"
    )

    try:
        with urllib.request.urlopen(req, timeout=120) as resp:
            data = json.loads(resp.read().decode("utf-8"))
            return data.get("message", {}).get("content", "")
    except urllib.error.URLError as e:
        sys.stderr.write(f"Error querying local agent at {endpoint}: {e}\n")
        sys.exit(1)

def main():
    parser = argparse.ArgumentParser(description="Query local Qwen model")
    parser.add_argument("--prompt", "-p", type=str, help="Prompt text")
    parser.add_argument("--file", "-f", type=str, help="Read prompt from file")
    parser.add_argument("--system", "-s", type=str, default="You are an expert Rust systems engineer working on a high-performance, genre-agnostic MMO server engine. Follow zero-warning strict Rust idiom, no unwrap in production paths, explicit bounds checking, and comprehensive unit tests.", help="System prompt")
    parser.add_argument("--model", "-m", type=str, default=DEFAULT_MODEL, help="Model name")
    parser.add_argument("--endpoint", "-e", type=str, default=DEFAULT_ENDPOINT, help="Ollama API endpoint")
    parser.add_argument("--output", "-o", type=str, help="Output file path (default stdout)")

    args = parser.parse_args()

    if args.file:
        with open(args.file, "r") as f:
            prompt = f.read()
    elif args.prompt:
        prompt = args.prompt
    else:
        prompt = sys.stdin.read()

    if not prompt.strip():
        sys.stderr.write("No prompt provided.\n")
        sys.exit(1)

    result = query_qwen(prompt, model=args.model, endpoint=args.endpoint, system=args.system)

    if args.output:
        with open(args.output, "w") as f:
            f.write(result)
    else:
        print(result)

if __name__ == "__main__":
    main()
