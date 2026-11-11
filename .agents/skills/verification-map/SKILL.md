---
name: verification-map
description: Create, use, or maintain a project-local verification skill (`verify-<app>`) with a feature map, so any agent can prove user-visible behaviour on the real running product. Use when a project has a user-facing surface (CLI, web, desktop, API, mobile) but no scripted way for an agent to drive it, when choosing how to verify a behaviour change or reproduce a reported bug, when promoting a successful drive into a registered check, or when an existing verify skill's feature map has drifted from the product.
---

# Verification Map

A verification skill is a short, project-local recipe for driving the real product the way a user does and capturing evidence. Its feature map splits that knowledge into one small file per feature, so an agent reads only what the change touches instead of one large document.

This skill supports the governance model; it does not change it. Registered checks in `checks.json` remain the only automatic gate. Behaviour proven through a verify skill is evidence only when cited as local evidence for the change.

## Create

Add the verify skill through a normal change packet whose `write_scope` covers the new paths.

1. **Interview the repository, not the owner.** Find out from code, scripts and docs, and ask only what cannot be observed. `references/platforms.md` gives the defaults per surface when the repository is silent:
   - **Surface:** what a user actually touches. Pick the primary one and note the others.
   - **Run:** the repository's own command to start it, plus ports, environment, seed data and auth.
   - **Drive:** how an agent can interact with it programmatically. Prefer an existing harness (test drivers, scripts, endpoints) over a new one.
   - **Observe:** what evidence can be captured: output, exit codes, files, responses, logs, screenshots, stored state.
   - **Isolate:** whether two instances can run side by side without sharing data, ports or profiles. If not, say so, and never drive an instance the user is using.
   If the product does not start as-is, fix that first or report it precisely. A recipe written against a broken base teaches wrong steps.
2. **Write `.agents/skills/verify-<app>/SKILL.md`** with `name` and `description` frontmatter and these sections, each filled from what the interview found:
   - **Launch:** the exact start command, how to tell it is ready, and teardown. For a short-lived CLI, launch means build or install once, then run each drive in a fresh, isolated working directory.
   - **Doctor:** one read-only check that the instance is worth driving: running, the right build, owned by this run.
   - **Drive:** real commands or selectors from this repository, not examples. Prefer stable handles (command names, labels, routes, data attributes) over positions.
   - **Evidence:** what to capture and where it is kept. Structured state is the oracle (accessibility tree or DOM with refs, status codes, logs, stored state); screenshots and recordings are evidence for people; OCR only where no accessibility data exists. Exercise the real user path, not internal setters or test-only shortcuts. Capture the action and the resulting state. Check side effects (files, stored data, messages), not only what is displayed. A mock counts only behind a boundary that already isolates a real external system.
   - **Cleanup:** stop only what this run started. Cleanup never deletes the evidence.
   Any helper script the skill ships must be executable, with its invocation shown in the skill.
3. **Add the adapter:** a relative symlink `.claude/skills/verify-<app>` → `../../.agents/skills/verify-<app>`. `repo.py status` checks it. Codex discovers `.agents/skills/` without one.
4. **Seed the feature map** in `.agents/skills/verify-<app>/features/`:
   - `README.md`: preconditions shared by every feature, the proof and skip rules, a full-sweep order for broad regression runs, and an index with one line per feature file.
   - One file per user-facing feature, starting with the three to five most important, following `references/feature-file.md`.
5. **Prove the skill once before relying on it:** launch, doctor, drive one mapped feature, capture evidence, clean up, then confirm the evidence still exists. A verify skill that was never executed is a draft.

## Use

- Match the change to the feature files it affects, and read only those.
- Run Doctor first, and again after any drive that failed or surprised you.
- Exercise every reachable entry point the feature file lists, and the success, cancel, error, empty and persistence paths the change can affect.
- When a path cannot be reached, name it and its prerequisite (account, platform, external state), and cover the closest real path that remains.
- For a broad regression, walk the README's sweep order.
- When delegating with the `change-execution` skill, point the brief's READ and VERIFY fields at the relevant feature files, so a worker or reviewer needs no knowledge of the whole product.

### Promote

Interactive driving is for exploring and reproducing. Once a drive has proven something worth re-checking, freeze it: save the exact steps as a replayable script that asserts on the oracle, register it in `checks.json` (its own group and CI job when the toolchain needs one; when CI cannot host the platform, keep it unregistered and cite the local run in the pull request), and point the feature file at it. `references/platforms.md`, "Explore, then freeze", has the steps. A reproduction from an `incident` change is promoted this way and becomes the fix's regression test.

## Maintain

The map drifts whenever the product changes. Update it in the same change as the product change when possible. Otherwise run a maintenance pass as its own change, with a `write_scope` limited to the verify skill's directory. Never edit product code in that pass.

1. **Index:** fix missing, extra, duplicate or dead entries in `features/README.md`.
2. **Source check:** for each feature file, compare it with the current source and note likely drift with file references. This can be split across read-only workers, one per feature file.
3. **Live pass:** drive every feature at least once, even when the source looks unchanged. Run Doctor before the first drive and after any failure, keep evidence through every cleanup, and leave nothing running.
4. **Sort each finding:**
   - **doc drift:** the map is wrong. Fix the map.
   - **harness gap:** the behaviour works but cannot be driven. Fix the harness, then drive it again.
   - **product gap:** the product is broken. Report it; keep it out of this change.

---

Adapts ideas from pstack's verification skills (MIT, Lauren Tan). The feature-map layout follows the structure of poteto's `verification-skill-example`, which is used as a structural reference only; no text is copied from it.
