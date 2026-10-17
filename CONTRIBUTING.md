# Contributing

**Read [`AGENTS.md`](AGENTS.md) first.** It is the canonical set of rules for
work in this repository: sources of truth, architectural boundaries, coding
standards, dependency policy, testing, security prohibitions, and the workflow to
follow. [`CLAUDE.md`](CLAUDE.md) adds Claude-specific notes and is subordinate to
it.

Short version of what will block a change:

- **Formatting and lint must pass.** Exact commands are recorded in
  `.github/workflows/ci.yml` and become valid once T-001 is verified.
- **Tests are written with the change,** covering invalid inputs as well as the
  happy path. Do not weaken, skip, or delete a test to make a build pass.
- **Report honestly.** Say which checks you ran, which passed, and which you did
  not run and why. Never write "tests pass" unless you ran them.
- **A new dependency needs a row in [`docs/dependency-register.md`](docs/dependency-register.md)**
  in the same change: purpose, licence, maintenance note.
- **Boundaries are not negotiable** — `AGENTS.md` §3. In particular the control
  plane never receives a local destination, the relay never dials
  client-chosen destinations, and secrets never enter the database, the
  repository, logs, or settings files.
- **Never** disable TLS or certificate validation, skip an authorization check,
  accept a local destination from a network message, or require administrator
  rights without a documented justification.

## Status

This repository is in the **planning phase**. There is no application code yet,
no test has been run, and the CI workflow has never been executed. Commands
recorded in documents are labelled *verified* only if they were actually run.
See [`README.md`](README.md) for the current state and [`review-report.md`](review-report.md)
for what has and has not been checked.

## Where things are defined

| Question | File |
|---|---|
| What must the product do? | [`requirements.md`](requirements.md) |
| What are the implementation steps? | [`tasks.md`](tasks.md) |
| How is the system structured? | [`architecture.md`](architecture.md) |
| What are the API and protocol contracts? | [`contracts.md`](contracts.md), [`api.md`](api.md) |
| What is stored? | [`data-model.md`](data-model.md), [`schema.md`](schema.md) |
| Why this design? | [`research.md`](research.md) (ADRs and experiments) |
| What is allowed in scope? | [`context.md`](context.md) |

If two documents disagree, fix the authoritative one first, then the companions,
and say so in your report. Never resolve a conflict silently.