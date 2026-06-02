# Contributing

Guidelines for subagents and contributors implementing this project.

---

## Scope

Each PR implements **exactly one issue** — nothing more, nothing less. Do not refactor adjacent code, add convenience APIs, or implement future-phase work while solving the current issue.

---

## Before Submitting

Every PR must pass all three without warnings:

```sh
cargo fmt --check
cargo clippy -- -D warnings
cargo test
```

---

## Code Style

- **Formatting**: `rustfmt` defaults. No manual formatting.
- **Naming**: follow Rust conventions — `snake_case` for functions/variables, `PascalCase` for types, `SCREAMING_SNAKE_CASE` for constants.
- **Imports**: group as `std` → external crates → crate-internal, separated by blank lines. No glob imports except in test modules (`use super::*`).
- **Line length**: 100 characters (configured in `rustfmt.toml` once added).

---

## Design Principles

- **No premature abstraction.** Three similar lines of code are better than a helper that only gets called three times. Add abstractions only when the duplication is real and the abstraction's boundary is obvious.
- **No defensive code.** Do not add fallbacks, retries, or validation for inputs that internal invariants already prevent. Validate only at system boundaries.
- **No dead code.** Do not leave `#[allow(dead_code)]` stubs for "future use". Implement what the issue asks; open a new issue if follow-up work is needed.
- **Correctness before performance.** Prefer the readable, obviously-correct implementation. Optimizations belong in Phase 3 issues and must be backed by a benchmark showing improvement.

---

## Comments

Write **no comments by default**. The only acceptable comment is one that explains *why* something non-obvious is true — a subtle invariant, a workaround for a specific constraint, or a counterintuitive choice.

```rust
// BAD — explains what the code already says
// increment the length counter
self.len += 1;

// GOOD — explains a non-obvious invariant
// len counts only keys present in leaves, not separator copies in internal nodes
self.len += 1;
```

For `unsafe` blocks, a `// SAFETY:` comment is **required** and must justify every precondition.

```rust
// SAFETY: idx < self.len is maintained by every insert path; slot is initialized.
unsafe { self.keys[idx].assume_init_ref() }
```

Public items (`pub`, `pub(crate)`) get a single-line doc comment. Multi-paragraph docstrings are not required.

---

## Testing

- Every new function introduced by an issue must have at least one test.
- Corner cases to always cover: empty tree, single element, min/max key values, operations that trigger splits or merges.
- After Phase 1 lands, `check_invariants()` must be called at the end of every test that mutates the tree.
- Property-based tests go in `tests/correctness.rs`. Unit tests go in an inline `#[cfg(test)]` module in the relevant source file.

---

## Unsafe

- Prefer safe Rust. Use `unsafe` only when a safe alternative would require cloning unnecessarily or impose measurable overhead on a hot path.
- Every `unsafe` block must be the smallest possible — wrap individual expressions, not entire functions.
- Document the invariant that makes the block safe, not just that it is safe.

---

## Module Ownership

Each file has a single responsibility. Do not reach into another module's internals — expose a function instead.

| File | Owns |
| ---- | ---- |
| `arena.rs` | Allocation and deallocation of nodes |
| `node.rs` | Node types and in-node operations (search, insert-at-index, split) |
| `tree.rs` | Tree-level algorithms (traversal, insert, delete) |
| `cursor.rs` | Range iteration over leaf nodes |
| `bulk.rs` | Bottom-up bulk construction |
| `stats.rs` | `TreeStats` collection and display |
| `invariants.rs` | `check_invariants` (test/debug builds only) |

---

## Phase Discipline

Phases exist to enforce the correctness-before-features order:

- **Phase 1 (Core)** — `NodeArena`, `InternalNode`, `LeafNode`, single-threaded insert/get/remove, invariant checker, property tests.
- **Phase 2 (Iterators)** — `Cursor`, range scan, `IntoIterator`, `FromIterator`, `Debug`, `TreeStats`.
- **Phase 3 (Optimization)** — bulk loader, `repr(align)`, benchmarks, const-generic `B` docs.
- **Phase 4 (Concurrency)** — `RwLock` wrapper, loom tests, OLC.

Do not implement a Phase N+1 item in a Phase N PR, even if it looks easy.
