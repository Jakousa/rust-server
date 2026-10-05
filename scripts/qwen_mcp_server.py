#!/usr/bin/env python3
"""
Lightweight MCP (Model Context Protocol) stdio server for local Qwen model on Ollama.
Exposes 'ask_qwen' tool to Antigravity (agy).
"""

import sys
import json
import urllib.request
import urllib.error

OLLAMA_ENDPOINT = "http://gateway.docker.internal:11434/api/chat"
MODEL = "qwen2.5-coder:14b"

def query_ollama(prompt: str, system: str = None) -> str:
    messages = []
    if system:
        messages.append({"role": "system", "content": system})
    else:
        messages.append({
            "role": "system",
            "content": "You are an expert Rust systems engineer working on a high-performance, genre-agnostic MMO server engine. Follow zero-warning strict Rust idiom, no unwrap in production paths, explicit bounds checking, and comprehensive unit tests."
        })
    messages.append({"role": "user", "content": prompt})

    payload = {
        "model": MODEL,
        "messages": messages,
        "stream": False,
        "options": {
            "temperature": 0.2,
        }
    }

    req = urllib.request.Request(
        OLLAMA_ENDPOINT,
        data=json.dumps(payload).encode("utf-8"),
        headers={"Content-Type": "application/json"},
        method="POST"
    )

    try:
        with urllib.request.urlopen(req, timeout=120) as resp:
            data = json.loads(resp.read().decode("utf-8"))
            return data.get("message", {}).get("content", "")
    except Exception as e:
        return f"Error communicating with local Qwen model: {e}"

def handle_message(msg):
    method = msg.get("method")
    msg_id = msg.get("id")

    if method == "initialize":
        return {
            "jsonrpc": "2.0",
            "id": msg_id,
            "result": {
                "protocolVersion": "2024-11-05",
                "capabilities": {
                    "tools": {}
                },
                "serverInfo": {
                    "name": "qwen-local",
                    "version": "1.0.0"
                }
            }
        }
    elif method == "notifications/initialized":
        return None
    elif method == "ping":
        return {"jsonrpc": "2.0", "id": msg_id, "result": {}}
    elif method == "tools/list":
        return {
            "jsonrpc": "2.0",
            "id": msg_id,
            "result": {
                "tools": [
                    {
                        "name": "ask_qwen",
                        "description": "Delegate coding tasks, boilerplate generation, wire codecs, SQL queries, or unit tests to the local Qwen 2.5 Coder 14B model via Ollama.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "prompt": {
                                    "type": "string",
                                    "description": "The detailed instructions and context for Qwen to generate code or tests."
                                },
                                "system_prompt": {
                                    "type": "string",
                                    "description": "Optional custom system prompt to guide Qwen's style or domain constraints."
                                }
                            },
                            "required": ["prompt"]
                        }
                    }
                ]
            }
        }
    elif method == "tools/call":
        params = msg.get("params", {})
        name = params.get("name")
        args = params.get("arguments", {})

        if name == "ask_qwen":
            prompt = args.get("prompt", "")
            system_prompt = args.get("system_prompt")
            output = query_ollama(prompt, system_prompt)
            return {
                "jsonrpc": "2.0",
                "id": msg_id,
                "result": {
                    "content": [
                        {
                            "type": "text",
                            "text": output
                        }
                    ]
                }
            }
        else:
            return {
                "jsonrpc": "2.0",
                "id": msg_id,
                "error": {
                    "code": -32601,
                    "message": f"Unknown tool: {name}"
                }
            }
    else:
        if msg_id is not None:
            return {
                "jsonrpc": "2.0",
                "id": msg_id,
                "error": {
                    "code": -32601,
                    "message": f"Method not found: {method}"
                }
            }
        return None

def main():
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            msg = json.loads(line)
            resp = handle_message(msg)
            if resp is not None:
                sys.stdout.write(json.dumps(resp) + "\n")
                sys.stdout.flush()
        except Exception as e:
            err_resp = {
                "jsonrpc": "2.0",
                "id": None,
                "error": {
                    "code": -32700,
                    "message": f"Parse error: {e}"
                }
            }
            sys.stdout.write(json.dumps(err_resp) + "\n")
            sys.stdout.flush()

if __name__ == "__main__":
    main()
