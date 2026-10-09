"""Agent adapters: run one attempt of a case in isolation and normalize its transcript.

`base` holds what every adapter shares (the normalized run record, the cap
watchdog, the isolated configuration directory, the allow-listed environment);
`claude_code` is the adapter of the agent under test (KTD2); a Codex adapter
would sit next to it. `judge_cli` is not an agent adapter: it runs the judge
CLI (KTD6) once, in the same isolation, and returns its reply and model.
"""
