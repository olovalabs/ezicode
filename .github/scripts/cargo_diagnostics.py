#!/usr/bin/env python3
"""Run Cargo normally and expose Rust errors/test failures as check annotations."""
import json
import os
import subprocess
import sys


def annotation(message, filename=None, line=None):
    message = message.replace("%", "%25").replace("\r", "%0D").replace("\n", "%0A")
    location = ""
    if filename:
        filename = os.path.relpath(filename).replace("%", "%25").replace(",", "%2C")
        location = f" file={filename},line={line or 1}"
    print(f"::error{location}::{message}", flush=True)


args = sys.argv[1:]
separator = args.index("--") if "--" in args else len(args)
args.insert(separator, "--message-format=json")
process = subprocess.Popen(["cargo", *args], stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
test_failure = []
for line in process.stdout:
    try:
        item = json.loads(line)
    except json.JSONDecodeError:
        print(line, end="", flush=True)
        if line.startswith("---- "):
            if test_failure:
                annotation("".join(test_failure)[:20000])
            test_failure = [line]
        elif test_failure:
            test_failure.append(line)
        continue
    if item.get("reason") == "compiler-message":
        diagnostic = item["message"]
        print(diagnostic.get("rendered", diagnostic["message"]), end="", flush=True)
        if diagnostic["level"] == "error":
            span = next((span for span in diagnostic["spans"] if span["is_primary"]), {})
            annotation(diagnostic.get("rendered") or diagnostic["message"], span.get("file_name"), span.get("line_start"))
status = process.wait()
if status and test_failure:
    annotation("".join(test_failure)[:20000])
sys.exit(status)
