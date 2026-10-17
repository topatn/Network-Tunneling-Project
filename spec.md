# spec.md — Specification Index

**Status:** DRAFT. Companion file. It contains **no requirement text**, so it cannot drift from `requirements.md`.

## Authoritative sources

| Question | Authoritative document |
|---|---|
| What must the product do, and how is it verified? | `requirements.md` |
| What are the implementation steps? | `tasks.md` |
| How is the system structured? | `architecture.md` |
| What are the API and protocol contracts? | `contracts.md`, `api.md`, `contracts/*.schema.json` |
| What is stored? | `data-model.md`, `schema.md` |
| What evidence supports decisions? | `research.md` |

## How feature specifications are organized

Features are specified by area. Each area's behavior lives in the documents above; this table says where to look.

| Feature area | Requirements | Contract | Design | Flow (planned) |
|---|---|---|---|---|
| Sign-in and sessions | FR-001, SEC-001 to SEC-003 | `contracts.md` §2, §5.1–5.4 | `architecture.md` §7 | `user-flows.md` |
| Tunnel lifecycle | FR-004 to FR-006, FR-014 | `contracts.md` §5.6–5.10, §12 | `architecture.md` §8 | `user-flows.md` |
| Invitations and guests | FR-007 to FR-009, SEC-007 | `contracts.md` §5.11–5.13, §6 | `architecture.md` §7 | `user-flows.md` |
| Forwarding (TCP/UDP) | NET-001 to NET-011 | `contracts.md` §10–11 | `architecture.md` §9–10 | n/a |
| Game profiles | GAME-001 to GAME-010 | `contracts.md` §5.5 | `architecture.md` §11 | `user-flows.md` |
| Host-local game server provisioning *(added 2026-10-17)* | SRV-001 to SRV-010 | `contracts.md` §13 (local commands; no API routes) | `architecture.md` §5, §11, §12, §18 | `user-flows.md` §5 (journey 5b) |
| Status and diagnostics | FR-010 to FR-013, UX-002 | `contracts.md` §13 | `architecture.md` §12 | `user-flows.md` |
| Security and abuse | SEC-004 to SEC-018 | throughout | `architecture.md` §18 | n/a |
| Operations and privacy | OPS-*, PRIV-*, REL-* | `contracts.md` §7 | `architecture.md` §15–17 | n/a |

## Rules

1. Change requirement text only in `requirements.md`; update `tasks.md` in the same change if the verification or scope changes.
2. A new feature starts as a requirement with acceptance criteria and a verification method, then gets tasks, then contracts and design updates.
3. Do not copy requirement text into other documents; link by ID.
4. Deferred items stay marked deferred (for example NET-011) until a decision gate in `plan.md` moves them.
