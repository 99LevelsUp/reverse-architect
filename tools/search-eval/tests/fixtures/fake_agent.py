"""Synthetic agent for process tests: speaks stream-json without any model.

Usage: fake_agent.py <mode> [arg ...] [ignored claude options ...]

Modes:
  replay <jsonl>          print the transcript's lines and exit 0
  spew <n> [<pidfile>]    emit n tool calls; with a pidfile also start a
                          sleeping child, write its pid there, then go silent
  tokens <n>              emit n messages of 1000 tokens each, then go silent
  hang                    emit the init event, then go silent (a CLI stuck at
                          login or on an unreachable API)
  env <dumpfile>         write environment, cwd, stdin and the config dir
                          listing as JSON to dumpfile, answer and exit 0
  preflight <behaviour>   answer the tool-check prompt (see `preflight`);
                          behaviour: good, leak-outside, follow-user,
                          extra-tool, extra-mcp, no-usage, skip-outside,
                          auth-error, bad-flag,
                          unresolved-outside (the outside Read gets no
                          tool result), empty-read-outside (the outside
                          Read with an offset past the end succeeds empty),
                          argerror-outside (the outside Read fails on its
                          arguments, not on confinement),
                          grep-outside (Read denied, then Grep on the
                          outside file succeeds), cap-after-progress (after
                          the outside Read, calls tools until the step cap
                          kills it; no result event)
  auto <mode> [arg ...]   answer a tool-check prompt as `preflight good`,
                          any other prompt as <mode> (spew, tokens, answer,
                          call, stash)
  answer <file> <member>  read nothing, answer with that location and exit 0
  call <tool> <file> <member>
                          call <tool> once, then answer like `answer`
  stash <file> <member>   write the prompt into the temp, cache and
                          configuration directories (as a CLI's own session
                          files would), then answer like `answer`

The modes answer, call and stash start with the init event Claude Code would
emit for the given options (see `init_event`).
"""
import json
import os
import re
import subprocess
import sys
import time

# Duplicates of preflight.PREFLIGHT_TITLE and preflight.OUTSIDE_LABEL (this
# script runs as a separate process); test_preflight keeps them equal.
PREFLIGHT_TITLE = "KONTROLA NÁSTROJE"
OUTSIDE_LABEL = "Soubor mimo pracovní strom: "
USAGE = {"input_tokens": 10, "cache_creation_input_tokens": 200,
         "cache_read_input_tokens": 300, "output_tokens": 40}
STASH_NAME = "stash-prompt.txt"

sys.stdout.reconfigure(encoding="utf-8")


def emit(obj):
    sys.stdout.write(json.dumps(obj, ensure_ascii=False) + "\n")
    sys.stdout.flush()


def assistant(message_id, content, usage=None, parent=None):
    message = {"id": message_id, "role": "assistant", "content": content}
    if usage is not None:
        message["usage"] = usage
    return {"type": "assistant", "message": message, "parent_tool_use_id": parent}


def tool_use(index):
    return assistant(f"msg_{index}", [{"type": "tool_use", "id": f"toolu_{index}",
                                       "name": "Read", "input": {"file_path": "a.txt"}}])


def go_silent():
    time.sleep(120)


def option(argv, name):
    return argv[argv.index(name) + 1] if name in argv[:-1] else None


def result(usage, is_error=False, text="hotovo"):
    event = {"type": "result", "subtype": "success", "is_error": is_error, "result": text}
    if usage is not None:
        event["usage"] = usage
    return event


def tool_result(tool_id, content, is_error=False):
    return {"type": "user", "parent_tool_use_id": None,
            "message": {"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": tool_id, "content": content,
                 "is_error": is_error}]}}


def answer_text(file, member, hypothesis):
    body = json.dumps({"file": file, "member": member, "hypothesis": hypothesis},
                      ensure_ascii=False)
    return "Závěr:\n```json\n" + body + "\n```"


def init_event(argv):
    """The init event Claude Code would emit for these options: the --tools, no MCP server."""
    tools = [t for t in (option(argv, "--tools") or "").split(",") if t]
    return {"type": "system", "subtype": "init", "tools": tools, "mcp_servers": [],
            "model": option(argv, "--model"), "claude_code_version": "0.0.0-fake"}


def preflight(argv, prompt, behaviour):
    """Play a Claude Code run of the tool-check prompt, honest unless told otherwise."""
    if behaviour == "bad-flag":
        sys.stderr.write("error: unknown option '--restricted'\n")
        return 1
    init = init_event(argv)
    init_tools, servers = init["tools"], init["mcp_servers"]
    if behaviour == "extra-tool":
        init_tools.append("Bash")
    if behaviour == "extra-mcp":
        init_tools.append("mcp__other__search")
        servers.append({"name": "other", "status": "connected"})
    emit(init)
    usage = None if behaviour == "no-usage" else USAGE
    if behaviour == "auth-error":
        text = "Invalid API key · Please run /login"
        emit(assistant("msg_err", [{"type": "text", "text": text}]))
        emit(result(usage, is_error=True, text=text))
        return 1
    readme = os.path.join(os.getcwd(), "README.txt")
    emit(assistant("msg_1", [{"type": "tool_use", "id": "t1", "name": "Read",
                              "input": {"file_path": readme}}], usage))
    emit(tool_result("t1", open(readme, encoding="utf-8").read()))
    outside = next((line[len(OUTSIDE_LABEL):].strip() for line in prompt.splitlines()
                    if line.startswith(OUTSIDE_LABEL)), None)
    if outside is not None and behaviour != "skip-outside":
        read_args = {"file_path": outside}
        if behaviour == "empty-read-outside":
            read_args.update(offset=100000, limit=1)
        emit(assistant("msg_2", [{"type": "tool_use", "id": "t2", "name": "Read",
                                  "input": read_args}], usage))
        if behaviour == "leak-outside":
            emit(tool_result("t2", open(outside, encoding="utf-8").read()))
        elif behaviour == "empty-read-outside":
            emit(tool_result("t2", ""))
        elif behaviour == "argerror-outside":
            emit(tool_result("t2", "InputValidationError: offset must be a number", True))
        elif behaviour != "unresolved-outside":
            # The refusal as Claude Code 2.1.287 words it under --restricted.
            emit(tool_result("t2", f"{outside} is outside {os.getcwd()}; --restricted "
                                   "confines the file tools to the working directory.", True))
        if behaviour == "grep-outside":
            emit(assistant("msg_2g", [{"type": "tool_use", "id": "t2g", "name": "Grep",
                                       "input": {"pattern": "^nic$", "path": outside}}], usage))
            emit(tool_result("t2g", "No matches found"))
        if behaviour == "cap-after-progress":
            for index in range(100):
                emit(tool_use(index))
            go_silent()
            return 0
    final = answer_text("README.txt", "Kontrola", "nedostupné")
    if behaviour == "follow-user":
        config_dir = os.environ.get("CLAUDE_CONFIG_DIR", "")
        text = open(os.path.join(config_dir, "CLAUDE.md"), encoding="utf-8").read()
        final += " " + re.search(r"PREFLIGHT-USER-\w+", text).group(0)
    emit(assistant("msg_4", [{"type": "text", "text": final}], usage))
    emit(result(usage))
    return 0


def answer(file, member):
    emit(assistant("msg_final", [{"type": "text", "text": answer_text(file, member, "h")}],
                   USAGE))
    emit(result(USAGE))
    return 0


def stash(prompt):
    """Leave the prompt where a CLI keeps session state: temp, cache, configuration."""
    for variable, sub in (("TMPDIR", ""), ("XDG_CACHE_HOME", "fake-cli"),
                          ("CLAUDE_CONFIG_DIR", "projects")):
        directory = os.path.join(os.environ[variable], sub)
        os.makedirs(directory, exist_ok=True)
        with open(os.path.join(directory, STASH_NAME), "w", encoding="utf-8") as fh:
            fh.write(prompt)


def main(argv, prompt=None):
    mode = argv[1]
    if mode in ("preflight", "auto"):
        prompt = sys.stdin.buffer.read().decode("utf-8")
        if mode == "preflight":
            return preflight(argv, prompt, argv[2])
        if PREFLIGHT_TITLE in prompt:
            return preflight(argv, prompt, "good")
        return main([argv[0], *argv[2:]], prompt)
    if mode in ("answer", "call", "stash"):
        emit(init_event(argv))
    if mode == "answer":
        return answer(argv[2], argv[3])
    if mode == "call":
        emit(assistant("msg_call", [{"type": "tool_use", "id": "t_call", "name": argv[2],
                                     "input": {"title": "kontrola"}}], USAGE))
        emit(tool_result("t_call", "ok"))
        return answer(argv[3], argv[4])
    if mode == "stash":
        stash(prompt if prompt is not None else sys.stdin.buffer.read().decode("utf-8"))
        return answer(argv[2], argv[3])
    if mode == "replay":
        with open(argv[2], encoding="utf-8") as fh:
            for line in fh:
                sys.stdout.write(line)
        sys.stdout.flush()
        return 0
    if mode == "spew":
        for index in range(int(argv[2])):
            emit(tool_use(index))
        if len(argv) > 3 and not argv[3].startswith("-"):
            child = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(120)"],
                                     stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                                     stderr=subprocess.DEVNULL)
            with open(argv[3], "w", encoding="utf-8") as fh:
                fh.write(str(child.pid))
        go_silent()
        return 0
    if mode == "tokens":
        usage = {"input_tokens": 100, "cache_creation_input_tokens": 400,
                 "cache_read_input_tokens": 400, "output_tokens": 100}
        for index in range(int(argv[2])):
            emit(assistant(f"msg_{index}", [{"type": "text", "text": "thinking"}], usage))
        go_silent()
        return 0
    if mode == "hang":
        emit(init_event(argv))
        go_silent()
        return 0
    if mode == "env":
        config_dir = os.environ.get("CLAUDE_CONFIG_DIR", "")
        dump = {
            "env": dict(os.environ),
            "cwd": os.getcwd(),
            "stdin": sys.stdin.buffer.read().decode("utf-8"),
            "argv": argv,
            "config_files": sorted(os.listdir(config_dir)) if config_dir else [],
        }
        with open(argv[2], "w", encoding="utf-8") as fh:
            json.dump(dump, fh, ensure_ascii=False)
        emit(assistant("msg_final", [{"type": "text", "text": "hotovo"}]))
        return 0
    raise SystemExit(f"unknown mode {mode}")


if __name__ == "__main__":
    sys.exit(main(sys.argv))
