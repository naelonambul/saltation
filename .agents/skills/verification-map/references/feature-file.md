# Feature file skeleton

Copy this into `features/<feature>.md` and replace every `<...>`. Keep it behaviour-level and short enough to run without reading source. Split the file when one section needs its own preconditions.

```markdown
# <Feature name>

<One sentence: what the feature does for the user.>

## Sub-features

- <sub-feature-id>: <what it does>

## How to get to it (user POV)

<The entry points a user takes: commands, menus, shortcuts, routes. List every one.>

## Driving it

<Exact commands or selectors from the verify skill's Drive section, per entry point, and the observable end state that proves each one: output, file contents, stored state.>

## Gotchas

- <What usually misleads: timing, stale state, ordering, platform differences, paths that need manual steps.>
```
