#!/usr/bin/env python3
"""Parsec plugin: a calculator. Also the reference for the plugin protocol.

Parsec starts this once and talks JSON, one object per line, on stdin.
Every message gets exactly one reply line on stdout.

  {"type": "init", "version": "0.1.0", "config": {...}}  ->  {"type": "ready"}
  {"type": "query", "id": 7, "text": "2+2", "keyword": "="}
      ->  {"type": "results", "id": 7, "items": [...]}
  {"type": "activate", "item": "...", "data": ..., "text": "..."}
      ->  {"type": "ok"}

An item: {"title", "subtitle"?, "icon"?, "id"?, "score"?, "actions": [...]}
An action: {"label", "open": url} | {"label", "copy": text}
         | {"label", "copy_secret": text} | {"label", "run": [argv]}
         | {"label", "callback": anything}   (sent back in an activate message)
"""
import ast
import json
import math
import operator
import sys

OPS = {
    ast.Add: operator.add, ast.Sub: operator.sub, ast.Mult: operator.mul,
    ast.Div: operator.truediv, ast.FloorDiv: operator.floordiv,
    ast.Mod: operator.mod, ast.Pow: operator.pow, ast.USub: operator.neg,
    ast.UAdd: operator.pos,
}
NAMES = {k: getattr(math, k) for k in dir(math) if not k.startswith("_")}
NAMES.update({"abs": abs, "round": round, "min": min, "max": max})


def evaluate(node):
    if isinstance(node, ast.Expression):
        return evaluate(node.body)
    if isinstance(node, ast.Constant) and isinstance(node.value, (int, float)):
        return node.value
    if isinstance(node, ast.BinOp) and type(node.op) in OPS:
        return OPS[type(node.op)](evaluate(node.left), evaluate(node.right))
    if isinstance(node, ast.UnaryOp) and type(node.op) in OPS:
        return OPS[type(node.op)](evaluate(node.operand))
    if isinstance(node, ast.Name) and node.id in NAMES:
        return NAMES[node.id]
    if isinstance(node, ast.Call) and isinstance(node.func, ast.Name) and node.func.id in NAMES:
        return NAMES[node.func.id](*[evaluate(a) for a in node.args])
    raise ValueError("unsupported expression")


def fmt(value):
    if isinstance(value, float) and value.is_integer() and abs(value) < 1e15:
        return str(int(value))
    if isinstance(value, float):
        return f"{value:.10g}"
    return str(value)


def results(text):
    expr = text.strip().replace("^", "**").replace(",", ".")
    if not expr:
        return []
    try:
        value = evaluate(ast.parse(expr, mode="eval"))
    except Exception:
        return [{"title": "…", "subtitle": f"{expr} is not a valid expression",
                 "icon": "dialog-question-symbolic", "actions": []}]
    shown = fmt(value)
    return [{
        "id": expr,
        "title": shown,
        "subtitle": f"{expr} =",
        "actions": [{"label": "Copy", "copy": shown}],
    }]


def main():
    for line in sys.stdin:
        try:
            msg = json.loads(line)
        except json.JSONDecodeError:
            continue
        kind = msg.get("type")
        if kind == "init":
            reply = {"type": "ready"}
        elif kind == "query":
            reply = {"type": "results", "id": msg.get("id"), "items": results(msg.get("text", ""))}
        else:
            reply = {"type": "ok"}
        sys.stdout.write(json.dumps(reply) + "\n")
        sys.stdout.flush()


if __name__ == "__main__":
    main()
