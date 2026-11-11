---
name: repository-quality
description: Establish and run repository-native quality tooling. Use when a project's language, framework, build system, or runtime becomes concrete and lint, format, test, type-check, or build checks are missing or incomplete, or during implementation, debugging, review, and completion when canonical validation must be discovered, bootstrapped, recorded, or run without replacing established conventions.
---

# Repository Quality

Treat quality tooling as repository infrastructure. Reuse established tooling; bootstrap only after the product stack is concrete.

## Determine the current state

1. Read `AGENTS.md`, `checks.json`, the root baseline, and the approved change packet relevant to the task.
2. Identify the language, framework, runtime, build system, and package manager from repository evidence or an explicit owner decision. Do not treat an exploratory prototype as a final stack decision.
3. Inspect repository-native command surfaces such as package scripts, build manifests, task runners, Makefiles, CI workflows, and existing scripts or config files.
4. Classify each applicable gate as established, partial, or absent: build, test, lint, format-check, type-check, boundaries, and project-specific verification.
5. If the product stack is still unresolved, do not install competing quality tools. Surface the missing decision instead.

## Bootstrap missing quality tooling

When the stack is established and suitable quality tooling is absent or materially incomplete:

1. Choose the smallest conventional toolchain that fits the selected ecosystem.
2. Prefer official, framework-native, or ecosystem-standard tooling and the repository's existing package/build manager.
3. Prefer one formatter and one linter. If one established tool safely covers both roles, avoid adding an overlapping second tool.
4. Declare tools as repository dependencies, plugins, or toolchain configuration. Do not depend on untracked global installation.
5. Keep configuration in the ecosystem's conventional location and ignore only generated or machine-local output.
6. Expose reproducible commands for the applicable gates. Include a non-mutating format check as well as a formatting command when the formatter supports both.
7. Preserve lockfiles and version constraints according to the ecosystem's normal package-management practice.
8. If the tooling setup would materially depart from the approved change, revise its plan and obtain a fresh owner approval before proceeding.
9. Run the new checks against the repository and fix starter configuration or source until the setup is genuinely usable. Do not weaken a rule merely to obtain a green result.
10. Register each gate as a check in `checks.json` (see below). If a stable multi-command wrapper improves reproducibility, put it under `scripts/` and register the wrapper.

## Stack-aware selection guidance

Use these as defaults only when no repository convention already exists:

- **JavaScript / TypeScript:** respect framework defaults first. Otherwise use the existing package manager with a conventional linter and formatter; add a TypeScript type-check command when TypeScript is part of the product stack.
- **Python:** prefer `pyproject.toml`-based configuration. A consolidated tool such as Ruff is a reasonable lint/format default when no convention exists; add the project's test runner and static type checker only when those gates are meaningful for the chosen stack.
- **Go:** prefer native `gofmt`, `go vet`, and `go test`; add an additional aggregate linter only when the project has a concrete reason for it.
- **Rust:** prefer Cargo-native `cargo fmt`, `cargo clippy`, and `cargo test`.
- **Swift / Xcode:** keep Xcode or SwiftPM build/test commands canonical. When adding style tooling from scratch, prefer established Swift ecosystem tooling and avoid redundant formatter/linter combinations.
- **Java / Kotlin:** integrate formatting and linting through the existing Gradle or Maven build rather than creating a parallel toolchain.
- **Other stacks:** follow the same rule: repository-native first, then the smallest official or ecosystem-standard toolchain that covers the applicable gates.

Do not replace an existing valid toolchain with one of these examples merely because it is newer or preferred by the agent.

## Preserve the checks

- Do not introduce a different formatter, linter, test framework, or type checker merely because you prefer it.
- Do not disable, weaken, skip, delete, or rewrite a failing check merely to make validation pass.
- Do not silently change test expectations to match broken implementation behavior.
- Change validation configuration only when the task actually requires that configuration change and the approved change authorizes it.

## Encode recurring mistakes

A mistake that has happened twice is a class, whether it surfaced in review, a revert, or an owner's correction. Nothing fails when an agent skips prose, so fix the class at the highest level that works, through a change packet:

1. **Architecture:** give the state one owner and the task one supported way, hide internals so the wrong import fails, and delete the old way an agent would copy.
2. **Types:** make the bad state impossible to write.
3. **A check:** use the stack's linter or boundary tool, registered in `checks.json`. Its failure must name the file, type, or function to use instead. If the pattern is already common, fail only on new occurrences.
4. **A test:** assert the behaviour, not the internals.
5. **Prose:** use it only for a judgment call. State in the plan why a higher level did not work.

Prove a new check by showing it fail on the real past mistake, and record that in the change's proof. Then delete any prose rule the check replaced.

Use a native tool for boundaries, such as Go `internal/`, Cargo workspaces, SwiftPM targets, `dependency-cruiser`, `import-linter`, or ArchUnit. Do not add import parsing to `repo.py`.

## Check registry contract

`checks.json` is the single list of canonical checks. `python3 scripts/repo.py verify` runs it locally and in CI. Each check declares:

- `id` and `group`;
- `argv`, an exact argument array (never a shell string), and `cwd`;
- `timeout_seconds`;
- `paths`, the changed paths that route to it. Unmapped changes run the full suite;
- `requires`, the executables it needs. A missing executable is `blocked`, never passed.

Keep the generic `core` group dependent only on Python and Git. A check that needs a stack toolchain (Node, JDK, Xcode, …) gets its own group and its own CI job in `.github/workflows/repository.yml`, which installs that toolchain and runs `repo.py verify --group <group>`. Add the job to the summary's `needs`. `repo.py status` fails when groups and jobs disagree. Groups are execution and isolation units only: a group-filtered run reports other checks as `not-run` and its evidence as incomplete, so a group can never stand in for the full required set.

The GitHub App cannot push `.github/workflows/**`, so a new group or CI job is an owner handoff; follow "Workflow changes" in `docs/host-setup.md`. Do not revert the group to silence `checks-unwired`. A check that needs only Python and Git can use `core`.

Never model a gate that does not apply as a skipped CI job. `repo.py` decides `not-applicable` from routing and records the reason.

## Validate proportionally

- During iteration, run the smallest relevant checks that provide fast feedback.
- Before completion, run every canonical check through `repo.py verify --change <id> --full`, which `close` requires, plus anything the approved plan requires.
- Treat visual or runtime verification as part of validation when the approved plan requires it.
- If meaningful validation cannot yet exist because the product stack is not established, state that fact instead of installing speculative tooling.

## Report evidence

Report the exact commands or checks run, whether they passed, the `.evidence/` path, and any relevant limitation. Do not claim a check ran when it did not. `blocked` and `not-run` are not passes.
