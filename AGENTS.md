# AGENTS.md — Instructions for AI Coding Agents

**Status:** DRAFT v0.1. Canonical for global agent workflow and coding rules. `CLAUDE.md` adds Claude-specific rules and defers to this file.
**Applies to:** every AI coding agent working in this repository.
**Last updated:** 2026-10-16

The project is in the **planning phase**. No production code exists. Commands below are conventions that become valid only when the task that creates them is verified (noted where it matters). Never claim a command works unless you ran it.

## 1. Mission and boundaries

Build a Windows desktop tool that lets people host game servers and share them without router configuration, as described in `context.md` and `architecture.md`. Work only on the task you were given (`tasks.md`). Do not expand scope, add deferred features (see `plan.md`, for example direct peer-to-peer, NET-011), or make broad architectural decisions inside a task.

## 2. Sources of truth

| Topic | Authoritative file |
|---|---|
| Agent workflow and coding rules | `AGENTS.md` |
| Claude-specific rules | `CLAUDE.md` |
| Product scope and boundaries | `context.md` |
| System architecture, component responsibilities | `architecture.md` |
| Domain and persistence model | `data-model.md` (physical schema: `schema.md`) |
| API and protocol contracts | `contracts.md` (inventory: `api.md`; schemas: `contracts/`) |
| Visual design | `DESIGN.md` |
| Interaction flows | `user-flows.md` |
| Requirements and acceptance criteria | `requirements.md` |
| Implementation checklist | `tasks.md` |
| Research evidence and ADRs | `research.md` |

If two documents disagree, fix the authoritative one first, then the companions, and say so in your report. Never resolve a conflict silently by choosing one reading.

## 3. Project structure and architectural boundaries

Planned layout (`architecture.md` §4; does not exist until T-001 and T-030):

- `apps/desktop/` UI only. No networking logic.
- `crates/engine/` tunnel client. `crates/relay/` relay. `crates/control-plane/` API. `crates/protocol/` shared messages. `crates/profiles/` profile loader. `profiles/` game profile data.
- `migrations/`, `infra/`, `tests/{integration,game-compat,network,security,perf}/`, `docs/`.

Boundaries that must not be crossed:

1. The control plane never stores or accepts a local destination (INV-1, ADR-003).
2. The relay never opens connections to destinations chosen by clients (SEC-005).
3. The engine dials only targets from its own configuration (SEC-006).
4. The UI never contains protocol logic; the protocol crate never depends on the UI.
5. Secrets never go in the database, repository, logs, or settings files.

## 4. Coding standards

- Follow the language's standard formatter and linter; the exact commands are recorded in CI config after T-001. Formatting and lint failures block merging.
- Rust naming follows the standard conventions (snake_case functions and modules, UpperCamelCase types). API and JSON field names are `snake_case`. Error codes are the lowercase codes in `contracts.md` §3. Mapping keys match `^[a-z][a-z0-9-]{0,31}$`.
- No panics, unbounded allocations, or unbounded loops on untrusted input (network data, files, user text). Bound every queue, buffer, and retry.
- Every network operation has a timeout. Every retry has a limit and backoff.
- Comments explain why, not what. Public items have documentation comments.
- Keep changes small and focused; do not reformat unrelated code.
- UI language and framework are **undecided** (ADR-005 follow-up); do not choose one inside an unrelated task.

## 5. Dependency policy

1. A new dependency needs a recorded purpose, license, and maintenance note in the dependency register (T-030) in the same change.
2. Prefer the standard library or an existing dependency. Do not add a dependency for a few lines of code.
3. Pin versions. Prefer releases that have been public for a while and whose changelogs you read; avoid abandoned or unmaintained packages.
4. CI runs a vulnerability scan (SEC-017). Do not suppress findings without a recorded justification.
5. Do not run install scripts from untrusted sources, and do not pipe downloaded scripts into a shell.

## 6. Testing and verification

1. Follow `tasks.md`: a task is complete only when its **Done when** condition is demonstrated by its stated verification. Code that exists is not complete.
2. Write tests with the change. Cover invalid inputs, not just the happy path (`TEST-001`).
3. Do not weaken, skip, or delete a test to make a build pass. If a test is wrong, fix it and explain why.
4. Tests must not contact production services or use production credentials.
5. **Report honestly:** list exactly which checks you ran, with results, and which you did not run and why. Never write "tests pass" unless you ran them in this session.
6. Compatibility claims for a game require its L2 real-client result (`research.md` ADR-011). Do not mark a game supported on the basis of a TCP connection or a status ping.
7. Experiments (M1) produce evidence in `research.md`, not production code. Scratch prototypes stay out of the production tree.

## 7. Security requirements and prohibited shortcuts

Never, for any reason, including "temporarily" or "for testing" in code that could ship:

- Disable TLS or certificate validation, or accept self-signed certificates outside tests.
- Hard-code or commit secrets, tokens, keys, or credentials. Do not print them to the terminal or logs.
- Skip an authorization check, or return `forbidden` where `contracts.md` requires `not_found`.
- Accept a local destination address or port from the relay, API, or any network message.
- Log tokens, invitation secrets, payload bytes, or raw guest IP addresses (PRIV-002, INV-5).
- Add backdoors, debug bypasses, or test-only accounts that exist in release builds.
- Require or request administrator privileges without a documented justification and a reviewed alternative (SEC-012).
- Enable insecure updater options, or bypass signature verification (SEC-014).
- Implement stealth, covert persistence, security-control evasion, or access to machines the user does not own or control.
- Expose services beyond what the host explicitly configured; optional mappings stay off by default (GAME-008).

If a task seems to require any of these, stop and report; do not proceed.

## 8. Documentation maintenance

- Update the authoritative document in the same change as the behavior it describes.
- When you change a requirement, update the mapped tasks in `tasks.md` and the traceability appendix.
- When you change an API or message, update `contracts.md`, the JSON Schema, `api.md`, and the example payloads, and re-run the example validation.
- When you change a table or column, update `data-model.md` and the migration together.
- Keep deferred features marked deferred. Do not describe planned behavior as existing.
- Mark commands and code in documents as verified only if you executed them; otherwise label them illustrative.

## 9. Terminal and tool-execution rules

1. **Inspect before you change.** Read the current content of a file (and `git status`/diff where applicable) before editing it. Do not overwrite work you did not make; merge instead.
2. Work inside the repository directory. Do not modify files outside it.
3. Prefer targeted edits over whole-file rewrites.
4. Prefer dry-run, preview, or read-only flags first for anything that changes state.
5. Do not use elevated privileges (`sudo`, administrator shells) unless the user explicitly instructs it for a stated purpose.
6. Do not install global packages or system software without asking.
7. Clean up background processes and temporary files you start.
8. Do not commit, push, rebase, or tag unless the user asks.

### Destructive commands and filesystem operations

Ask for explicit confirmation, naming the exact command and target, before: recursive or forced deletion, `git reset --hard`, `git clean`, force-push, history rewriting, dropping or truncating databases, overwriting migrations that have been applied, disk or partition operations, or anything that cannot be undone. Never run such commands against production resources. Never edit an applied migration; add a new one.

## 10. Handling uncertainty and conflicting requirements

- Distinguish in your notes: **verified fact**, **reasoned recommendation**, **assumption**, **needs experiment** (the labels used in `research.md`).
- Do not invent benchmarks, specifications, library behavior, compatibility results, or costs. Look them up in current official documentation, or record the gap.
- If a requirement is ambiguous or two documents conflict, stop, state the conflict, propose options, and ask. If the decision is architectural, record it as an ADR candidate in `research.md`.
- If you discover that an assumption in the docs is wrong, report it and propose the correction; do not work around it quietly.

## 11. Workflow

**Plan.** Read the task, its requirements, and the linked contract and architecture sections. State the plan and the verification you will run. Confirm prerequisites are verified.

**Implement.** Make the smallest change that satisfies the task. Write tests with it. Keep the repository building.

**Review.** Re-read your diff for security issues (section 7), boundary violations (section 3), and unintended changes. Re-run verification.

**Complete.** Tick the task in `tasks.md` only after its verification passed. Report: what changed, which checks ran with results, which checks did not run, open questions, and any documents updated.

## 12. Links

`CLAUDE.md`, `tasks.md`, `requirements.md`, `architecture.md`, `contracts.md`, `research.md`, `plan.md`.
