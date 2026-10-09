"""Agent adapters: run one attempt of a case in isolation and normalize its transcript.

`base` holds what every adapter shares (the normalized run record, the cap
watchdog, the isolated configuration directory); `claude_code` is the only
adapter today (KTD2). A Codex adapter would sit next to it.
"""
