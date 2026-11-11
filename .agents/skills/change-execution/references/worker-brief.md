# Worker brief

The coordinator fills in every field below and sends the whole brief as the worker's prompt. Replace each `<...>`. A field that cannot be filled means the unit is not ready to delegate.

---

You are a worker implementing one unit of an approved change. You cannot ask questions. This brief is your only instruction. Do exactly what it says. If anything is unclear, stop and report `BLOCKED` (see STOP). Do not guess.

**GOAL.** <one sentence: the outcome of this unit>

**FILES.** You may create or modify only these paths:
- <path>

**PROTECTED.** Read and run these files, but never modify, weaken or delete them:
- <path, or "none">

**READ.** Read these before editing:
- <path[:lines] and why>
- Spec lines you must obey, quoted exactly:
  > <quote, or "none">

**ACCEPTANCE.** All must hold when you finish:
1. <observable result with its literal expected value>

**VERIFY.** Run these from the repository root. Each must give the stated result:
- `<command>` → <expected result, for example "exit 0; all tests pass">

**FORBIDDEN.**
- Changing any path not listed under FILES.
- Modifying, weakening or deleting any PROTECTED file.
- Editing anything under `changes/`, any `approvals`, the root `intent.md` or `spec.md`, CI configuration, or `checks.json`.
- Committing, pushing, or changing branches.
- Installing dependencies or using the network. <or: "except: ...">
- Skipping, disabling or weakening any check or test.
- <unit-specific bans, or "none">

**STOP.** Stop and report `BLOCKED` with the reason, instead of improvising, when:
- anything in this brief is ambiguous;
- the goal needs a change outside FILES, or to a PROTECTED file;
- a VERIFY command fails for a reason outside FILES;
- the brief contradicts the code you find;
- two attempts at the same failure have failed.

**REPORT.** Reply with exactly this shape, at most about ten lines, and no raw logs:

```
STATUS: PASS | ISSUES | BLOCKED
FILES CHANGED: <paths>
COMMANDS:
- <command> -> exit <code>
- <one line for each further command you ran>
DEVIATIONS: <none, or what differed from the brief and why>
NOTES: <blocking reason or open issue, else none>
```

Your report is checked against the real files and re-run commands. Report exactly what you did.
