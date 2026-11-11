# Platform defaults for a verify skill

Defaults for the Launch, Doctor, Drive and Evidence sections of a `verify-<app>` skill, by the kind of surface the product has. Use them when the repository gives no better answer. Every tool named here is an example; the product's own harness, scripts and conventions win.

## One rule for observation

Read structured state as the oracle. Capture pixels as evidence for people.

- **Oracle** (what decides pass or fail): an accessibility tree or DOM with stable element references, HTTP status and body, exit codes, log lines, stored state, files written. These are deterministic, cheap in context, and survive restyling.
- **Evidence for people**: screenshots, screen recordings, traces. Attach them to the pull request; do not make the agent's decision depend on reading them.
- **OCR** of a screenshot is a last resort, only where no accessibility data exists (custom-rendered canvases, games, some legacy desktop toolkits). When it is used, say so in the feature file, because it is slow and misreads.

Published agent tooling for web, mobile and desktop converges on this split, and benchmark results agree: structured observation alone beats screenshots alone by a wide margin, and the combination is best.

## Defaults by surface

| Surface | Launch and Doctor | Drive | Oracle | Evidence for people | Tools, for example | Runs in CI? |
|---|---|---|---|---|---|---|
| Backend / API | start the service with the repository's command against an isolated database or fixture; Doctor is a health endpoint or a ready log line | HTTP or RPC calls, the repository's test client, queue or job triggers | status codes, response bodies, log lines, stack traces, rows written | request and response transcript, trace link | `curl`, the framework's test client, error-tracker CLIs or MCP servers for production context | yes |
| Web | start the dev or preview server; Doctor is a `200` on the root route | a browser automation library through accessibility snapshots with element refs | accessibility snapshot, DOM assertions, network responses, console errors | screenshot, trace | Playwright (MCP or scripted) | yes, headless |
| Desktop | launch the built app in a fresh profile or data directory; Doctor confirms the window and the build id | OS accessibility API: UI Automation on Windows, AXUIElement on macOS, AT-SPI on Linux; keyboard and pointer only through those APIs | accessibility tree, app logs, files and preferences the app writes | screenshot | the platform's accessibility tooling or an agent-facing wrapper over it | usually local or a dedicated runner |
| Mobile | boot a named simulator or emulator, install the build, launch; Doctor confirms the device id and the app is foreground | accessibility tree with refs, taps and text by ref | accessibility tree dump, app and device logs, stored state | simulator screenshot, recording | Maestro (MCP or flows), agent-device, sim-use, `xcrun simctl`, `adb` and `uiautomator` | Android often yes; iOS needs a macOS runner, usually local |
| CLI | build or install once; each drive runs in a fresh temporary working directory | invoke the command with arguments and stdin | exit code, stdout and stderr, files written | transcript | the shell | yes |

Notes:

- **Isolate.** Two drives must never share a profile, port, database or device. Claim a device or port per run and record it in Doctor, so parallel agents do not take over each other's simulators.
- **Settle, then read.** Read the oracle after the UI or service has settled (an element appears, a request completes), never after a fixed sleep.
- **Real path.** Drive the entry points a user takes. A test-only setter or an internal endpoint proves less; a mock counts only behind a boundary that already isolates a real external system.

## Explore, then freeze

An agent finds a path interactively: inspect state, choose the next command, repeat. That is right for reproducing a report and for a first pass over a feature. It is wrong as a regression test, because it is not deterministic and it costs model time on every run.

When a drive has proven something worth re-checking, promote it:

1. **Save** the exact drive as a replayable script in the repository's conventional place (a Maestro flow, a Playwright test, an agent-device session, a shell script with assertions). The script asserts on the oracle, not on a screenshot.
2. **Register** it in `checks.json` with its own group and, if the toolchain is not Python and Git, its own CI job that installs that toolchain, following the `repository-quality` skill. From then on `repo.py verify` runs it and routes changed paths to it.
3. **Keep it local** when CI cannot afford or host it (iOS simulator, a desktop app, a device farm). Do not register it in `checks.json`: `close` needs every registered check `passed`, and a `blocked` check fails CI. Keep the script in the repository, run it locally, and cite the result in the pull request.
4. **Point the feature file** at the script, so the next agent replays before it explores.

A reproduction drive from an `incident` change is promoted the same way, which is how the fix gets its regression test.
