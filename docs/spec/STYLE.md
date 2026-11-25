# How this specification is written

This document defines how `docs/spec/` is written. It applies to every
change to the spec, whoever makes it, human or agent.

## Purpose and audience

The spec states what Hamon **guarantees**. It is written for anyone who needs
to know what Hamon does without reading its code: a project manager deciding
whether Hamon fits a project, a reviewer checking a change, a developer
relying on a behaviour.

It is not:

- **the guide** (`docs/guide/`), which teaches developers how to use the API
  with compiled examples;
- **a description of the implementation**, which lives in the code and in
  `COVERAGE.md`.

The test for every sentence: *would it still be true after a full rewrite
of Hamon's internals?* If not, it doesn't belong in the spec.

## Structure

The spec reads as one document, in the order a user discovers Hamon.
`docs/spec/README.md` introduces Hamon and lists the chapters; each chapter
is a numbered file (`01-…`, `02-…`).

Each chapter:

1. **Introduces the concept** before stating any guarantee: what it is and
   why it exists, in a few sentences.
2. **States the guarantees** in short paragraphs, grouped by what a reader
   would ask ("when does a listener fire?", "in what order?").
3. **Defines terms on first use**, and uses them consistently afterwards.
   Don't say "row" in one paragraph and "record" in the next.

No heading per rule. Headings follow the reader's questions, not the list of
requirements.

## Voice

- **Plain words, short sentences.** Explain before you specify.
- **Present tense, declarative.** "The listener fires once", not "the
  listener should fire once" or "the listener will fire once".
- **Active voice where it helps:** say who does what ("Hamon recomputes…",
  "the listener receives…").
- **One idea per paragraph.** A paragraph is the unit that carries ids, so
  keep it focused enough that its ids are obvious.

## Precision

Readable never means vague. Every guarantee is stated with the exact words
that matter, and these words have fixed meanings:

| Word | Meaning |
|---|---|
| exactly once | one time, never zero, never more |
| at most once | zero or one time |
| at least once | one or more times |
| before / after | ordering that a user can observe and rely on |
| unspecified | Hamon makes no promise; don't rely on it (e.g. the order of returned rows) |
| may | permitted, not guaranteed |
| never | a guarantee, not a tendency |

When you rewrite a paragraph, compare it with the text it replaces and check
that no guarantee was lost or weakened ("exactly once" must not become
"once").

## Requirement ids

Every guarantee carries a stable id, such as `HMN-REACT-007`. Tests, issues,
commits and `COVERAGE.md` cite these ids, so they must never break.

**Tagging.** A paragraph ends with the ids it defines, inside a `<sub>` tag on
the same line as the last sentence:

```markdown
A listener runs after Hamon has released the row, so it can safely write to
that same row. <sub>HMN-REACT-007</sub>
```

Several ids in one paragraph are separated by ` · `. Each id is defined in
exactly one paragraph. Mentioning an id elsewhere ("see HMN-REACT-003") is a
reference, not a definition, and does not use the `<sub>` tag.

**Stability.**

- Ids are never renamed, renumbered or reused.
- Ids are opaque: the area in the name (`REACT`, `COMP`…) is historical and
  says nothing about which chapter the paragraph lives in. Paragraphs may move
  between chapters; their ids move with them.
- New guarantees get a new id with the next free number in the closest area.
- A guarantee Hamon no longer makes is **retired**, not deleted: its id moves
  to the "Retired requirements" list at the end of `README.md`, with one line
  saying why and a link to the PR that retired it.

`scripts/check-spec-ids.sh` enforces this: no id defined twice, and no id from
`main` missing from the spec unless it is retired.

## What stays out of the spec

- **File paths, line numbers, internal type and function names**
  (`store.rs:150`, `catch_unwind`, `Arc<RecomputeQueue>`). They go in the
  `Code` column of `COVERAGE.md`, on the row of the id they belong to.
  Public API names (`db.update`, `register_listener`) are fine: users see them.
- **Code examples.** The spec is not compiled, so examples would drift. Link
  to the relevant page of `docs/guide/` instead.
- **Test coverage.** That's `COVERAGE.md`.

## Rationale

When knowing *why* helps the reader, add it right after the guarantee as a
blockquote starting with `**Why:**`. Rationale explains, it doesn't promise:
it carries no id and may change without changing the contract.

```markdown
> **Why:** releasing the row first means a listener can react to a change by
> writing again, without deadlocking on its own row.
```

## Open questions

Behaviour that exists but whose intent isn't settled is marked where it
occurs, as a readable callout. It keeps the `OPEN(…)` marker so tooling
finds it, and links to the issue where it is decided:

```markdown
> **Open question** — OPEN(HMN-REACT-005, #7): a background recompute notifies
> the listener even when the recomputed row is identical, unlike a direct
> update. Should both behave the same?
```

Write the question so a non-developer understands what is being decided.
Resolving it means removing the callout and rewriting the paragraph to state
the settled behaviour, in the PR that implements the decision.

## Checklist for every spec change

- [ ] Concepts introduced before guarantees; terms defined on first use
- [ ] Every guarantee is precise (see the vocabulary table) and none was weakened
- [ ] Every id defined exactly once, with a `<sub>` tag; none renamed or dropped
- [ ] No implementation details; code references moved to `COVERAGE.md`
- [ ] Rationale in `**Why:**` blocks, open questions as callouts with an issue link
- [ ] `scripts/check-spec-ids.sh` passes
