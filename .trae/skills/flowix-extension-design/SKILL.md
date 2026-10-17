---
name: "flowix-extension-design"
description: "Guides creation of Flowix design documents (HLD/LLD) for new subsystems. Invoke when designing a module/feature to add to the Flowix codebase, extending an existing domain, or producing an architecture/design doc."
---

# Flowix Extension Design

For producing a structured, code-grounded design document when adding a new module or feature to the Flowix workspace. The output is a Markdown design doc under `docs/` following the repository's `UPPERCASE-DASHED.md` naming convention (e.g. `docs/AGENT-PROFILE-MODEL.md`).

## When to invoke

- The user asks to "设计" (design) a Flowix subsystem, module, pipeline, or feature.
- A previous planning document (e.g. an `*-PLAN.md`) identifies a domain needing detailed design.
- The task is to turn a product requirement or domain mapping into an implementation-ready design doc.

Do **not** use this skill for PRDs, competitive analyses, or user-facing manuals — those have different reference styles.

## Workflow

### 1. Verify code facts before writing

Never design against assumptions. Before drafting, run targeted `Grep`/`Read` to confirm:

- The actual public API signatures the design will call or extend.
- The real enum variants / struct fields that affect filtering, routing, or validation.
- Whether an existing facility already covers part of the need (this often raises the real completion %).
- The exact path conventions (e.g. `.flowix/` is local-only vs synced Markdown notes).

Record the verified facts; the design must cite real file paths with line anchors where it builds on existing code.

### 2. Requirement traceability table

Open with a table mapping each requirement `R1..N` to its source (upstream plan doc, a prior decision, or derived in this design), using evidence tags:

- `[Data-backed]` — verifiable from an existing artifact or a user-confirmed decision.
- `[Hypothesis]` — an assumption the design depends on; call out that it must be confirmed before implementation.
- `[Expert judgment]` — a design choice you are making.

Also list the explicit design decisions made to close open questions (e.g. `D-N1: expertise = full replacement`).

### 3. Architecture diagram + structural explanation

Include one Mermaid diagram showing components and data flow. After it, give **2–3 sentences** on the structural properties that make the design safe (e.g. "there is exactly one exit point past the confirmation gate", "write-back events are filtered by path so no feedback loop"). Avoid meta-narration.

### 4. Component detailed design

For each component, give: a one-sentence responsibility; input / output; boundary (what it does **not** do); a small pseudocode block for non-trivial logic; failure semantics. If a component reuses an existing module, state exactly what it reuses and what it adds.

### 5. Interface contracts

- Rust struct signatures and IPC command tables (command / params / return / error codes).
- For JSON contracts, include one concrete example and a field constraint table.
- Error codes should be small lowercase snake_case strings, matching the repo's stable machine-code style.

### 6. Data plane

Table of all new files: path, content, sync vs local, lifecycle. Justify why each lives where it does (local state under `<nb>/.flowix/`, synced identity as a real note, device-level config under `~/.flowix/`).

### 7. Non-functional + integration impact

Security/cost/concurrency/privacy as they apply. A change-point table listing every file touched and whether the change is invasive or additive.

### 8. Implementation steps

3–4 incremental milestones, each named (e.g. `N1`, `S-M2`) with a measurable acceptance sentence.

### 9. Test anchors

A table of ~10–12 integration test cases (input → expected outcome) covering the happy path, each failure path, adversarial/security cases, and compatibility cases (e.g. old manifest versions).

### 10. Open items

Remaining `[To be confirmed]` decisions and `[Hypothesis]` items that gate implementation.

## Conventions

- Doc header: `> 状态：设计稿 vX.Y（…，待评审）｜ YYYY-MM-DD`
- Doc title: `<Domain> <title>设计（详细版）` for LLD; plain for HLD.
- Headings end without punctuation. No emoji.
- Bold for tool/command names and critical warnings only (≤ 3 per section).
- No "确保" followed by an unverifiable goal — replace with a measurable criterion.
- No "不是…而是…" rhetorical frames; state the decision directly.
- Mermaid for diagrams; walk each diagram in prose immediately after it.
- Evidence tags appear in the traceability table and at key design decisions, not scattered through prose.
