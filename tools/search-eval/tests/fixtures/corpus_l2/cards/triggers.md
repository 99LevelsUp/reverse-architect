---
schema: 7
tool: reverse-architect
kind: triggers
---

# Triggers

## Part 1

- trigger · OnSave · active → `AcmeTrigger.Fire()` — `src/Acme/Jiny.cs:836-845`
- trigger · OnDelete · active → `ExternalCheck.Run()` — `src/Acme/Models/ExternalCheck.cs:34`

## Part 2

- event · Closed · dormant → `AcmeTrigger.Fire()` — `src/Acme/Jiny.cs:870`
