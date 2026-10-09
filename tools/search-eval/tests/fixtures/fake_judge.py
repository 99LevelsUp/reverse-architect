"""Synthetic judge CLI for process tests: answers like a judge, without any model.

Usage: fake_judge.py <shape> <mode> [arg ...] [ignored CLI options ...]

shape   claude  prints one `claude -p --output-format json` result object
        codex   prints `codex exec --json` events and writes the session file
                with the model into $CODEX_HOME/sessions (as codex does)

mode    script <file> <counter>
            <file> is JSON: {"model": "...", "smoke": "ok|leak|notjson|nomodel",
            "odpovedi": [...]}. A prompt that starts the smoke test (title
            KONTROLA HODNOTITELE) is answered by `smoke`; every other prompt gets
            the next entry of `odpovedi`, counted in the file <counter>
            (created at the first call). An entry is the answer JSON object
            itself, or {"_raw": text} for a verbatim reply text, and may carry
            "_model" (reported model for that call) or "_rc" (exit with that
            code and print nothing). Running out of entries exits with 1.
        nojson      print a line that is no JSON, exit 0
        rc1         complain on stderr, exit 1
        dump <file> write the environment, cwd, cwd listing, sibling listing and
            the configuration directory listing as JSON to <file>, then answer
            the smoke test; the model is `dump-model`
        hang        stay silent for two minutes

The "leak" smoke behaviour reads the file `canary.txt` next to the working
directory's sibling (the place the smoke test plants it) and prints its
content, like a judge that is able to read outside its working directory.
"""
import json
import os
import sys
import time

SMOKE_TITLE = "KONTROLA HODNOTITELE"
CANARY_FILE = "canary.txt"
DEFAULT_MODEL = "fake-model-1"

sys.stdout.reconfigure(encoding="utf-8")


def emit_reply(shape, text, model):
    if shape == "claude":
        result = {"type": "result", "subtype": "success", "is_error": False, "result": text}
        if model is not None:
            result["modelUsage"] = {model: {"inputTokens": 1}}
        print(json.dumps(result, ensure_ascii=False))
        return
    home = os.environ.get("CODEX_HOME")
    if home and model is not None:
        directory = os.path.join(home, "sessions", "2026", "01", "01")
        os.makedirs(directory, exist_ok=True)
        with open(os.path.join(directory, "rollout-fake.jsonl"), "w", encoding="utf-8") as out:
            out.write(json.dumps({"type": "session_meta", "payload": {"id": "x"}}) + "\n")
            out.write(json.dumps({"type": "turn_context", "payload": {"model": model}}) + "\n")
    for event in ({"type": "thread.started", "thread_id": "t"}, {"type": "turn.started"},
                  {"type": "item.completed", "item": {"id": "i", "type": "agent_message",
                                                      "text": text}},
                  {"type": "turn.completed", "usage": {}}):
        print(json.dumps(event, ensure_ascii=False))


def listing(path):
    if not os.path.isdir(path):
        return None
    found = []
    for base, _dirs, files in os.walk(path):
        found += [os.path.relpath(os.path.join(base, f), path).replace(os.sep, "/")
                  for f in files]
    return sorted(found)


def smoke_reply(kind):
    if kind == "leak":
        sibling = os.path.join(os.path.dirname(os.getcwd()), "sibling", CANARY_FILE)
        with open(sibling, encoding="utf-8") as handle:
            return json.dumps({"stav": "OK", "obsah": handle.read()})
    if kind == "notjson":
        return "ahoj, nevím"
    return json.dumps({"stav": "OK", "obsah": ""})


def main(argv):
    shape, mode, rest = argv[0], argv[1], argv[2:]
    prompt = sys.stdin.read()
    if mode == "nojson":
        print("tohle není JSON")
        return 0
    if mode == "rc1":
        sys.stderr.write("simulovaná chyba hodnotitele\n")
        return 1
    if mode == "hang":
        time.sleep(120)
        return 0
    if mode == "dump":
        config = os.environ.get("CLAUDE_CONFIG_DIR") or os.environ.get("CODEX_HOME")
        cwd = os.getcwd()
        with open(rest[0], "w", encoding="utf-8") as out:
            json.dump({"env": dict(os.environ), "cwd": cwd, "cwd_listing": listing(cwd),
                       "sibling_listing": listing(os.path.join(os.path.dirname(cwd), "sibling")),
                       "config_dir": config, "config_listing": listing(config) if config else None,
                       "prompt": prompt}, out, ensure_ascii=False)
        emit_reply(shape, smoke_reply("ok"), "dump-model")
        return 0
    if mode != "script":
        sys.stderr.write(f"neznámý režim {mode}\n")
        return 2
    with open(rest[0], encoding="utf-8") as handle:
        script = json.load(handle)
    model = script.get("model", DEFAULT_MODEL)
    if SMOKE_TITLE in prompt:
        kind = script.get("smoke", "ok")
        emit_reply(shape, smoke_reply(kind), None if kind == "nomodel" else model)
        return 0
    counter = rest[1]
    index = int(open(counter, encoding="utf-8").read()) if os.path.isfile(counter) else 0
    with open(counter, "w", encoding="utf-8") as handle:
        handle.write(str(index + 1))
    answers = script["odpovedi"]
    if index >= len(answers):
        sys.stderr.write("skript hodnotitele došel\n")
        return 1
    entry = answers[index]
    if isinstance(entry, dict) and "_rc" in entry:
        return int(entry["_rc"])
    reported = entry.pop("_model", model) if isinstance(entry, dict) else model
    text = entry["_raw"] if isinstance(entry, dict) and "_raw" in entry else json.dumps(
        entry, ensure_ascii=False)
    emit_reply(shape, text, reported)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
