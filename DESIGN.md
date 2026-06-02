# B+Tree — Design Document

**Status:** Finalized  
**Last Updated:** 2026-06-02

---

## 1. Overview

This project is a high-performance, in-memory B+Tree library written in Rust. The goal is not merely a correct B+Tree, but one that is competitive with production-grade index structures — optimized for cache efficiency, concurrent access, and bulk workloads.

The library will be published as a standalone Rust crate with a clean, ergonomic API. It is designed to serve as a production-capable component.

---

## 2. Goals

| Goal | Description |
| ---- | ----------- |
| **Correctness** | All operations must satisfy B+Tree invariants at all times |
| **Performance** | Throughput and latency competitive with `BTreeMap` from std and `indexmap` |
| **Cache efficiency** | Node layout aligned to CPU cache lines; arena allocation to minimize pointer chasing |
| **Concurrency** | Thread-safe with low contention via fine-grained locking (OLC) |
| **Ergonomics** | API that feels natural to Rust users, with full iterator support |
| **Observability** | Stats and introspection APIs for debugging and benchmarking |

## 3. Non-Goals (v1)

- Persistence / disk-based storage (intentionally deferred — architecture allows layering it later)
- MVCC or snapshot isolation
- Distributed operation
- SIMD-accelerated key comparison (stretch goal, not required for v1)

---

## 4. Background: B+Tree Properties

A B+Tree of order `B` satisfies:

1. Every internal node has between `⌈B/2⌉` and `B` children (except root, which has 2–B).
2. All data lives in leaf nodes; internal nodes hold only separator keys.
3. Leaf nodes are linked in a doubly-linked list, enabling efficient range scans.
4. The tree is perfectly height-balanced at all times.
5. Search, insert, and delete are all `O(log_B n)`.

The choice of `B` is critical for performance. For in-memory operation, `B` should be chosen so that a single node fills one or two cache lines (64–128 bytes). With 8-byte keys, this means `B ≈ 8–16`.

---

## 5. Architecture

```text
┌─────────────────────────────────────────────┐
│                  BPlusTree<K, V>             │  ← Public API
│  ┌──────────┐  ┌──────────┐  ┌───────────┐  │
│  │  Insert  │  │  Delete  │  │  Search   │  │
│  └──────────┘  └──────────┘  └───────────┘  │
│                                              │
│  ┌──────────────────────────────────────┐   │
│  │            NodeArena                 │   │  ← Memory layer
│  │  (pool allocator, NodeId handles)    │   │
│  └──────────────────────────────────────┘   │
│                                              │
│  ┌───────────────┐   ┌────────────────────┐ │
│  │  InternalNode │   │     LeafNode       │ │  ← Node types
│  │  [keys | ptrs]│   │  [keys | vals | →] │ │
│  └───────────────┘   └────────────────────┘ │
└─────────────────────────────────────────────┘
```

### Key Components

| Component | Responsibility |
| --------- | -------------- |
| `BPlusTree<K, V>` | Public-facing tree; owns the arena and root |
| `NodeArena` | Slab allocator; returns `NodeId` handles (u32 indices into a Vec) |
| `Node` | Enum over `InternalNode` and `LeafNode` |
| `InternalNode` | Stores separator keys and child `NodeId`s |
| `LeafNode` | Stores keys, values, and prev/next `NodeId` links |
| `Cursor` | Iterator over leaf nodes for range scans |

---

## 6. Data Structures

### 6.1 NodeId

```rust
#[derive(Copy, Clone, PartialEq, Eq)]
struct NodeId(u32);  // index into arena; u32 keeps node size small
```

Using indices (not pointers) means the arena can be a plain `Vec<Node<K,V>>`. This eliminates pointer indirection, keeps node sizes predictable, and is friendly to the allocator.

### 6.2 InternalNode

```rust
struct InternalNode<K> {
    len: u16,                   // number of keys (children = len + 1)
    keys: [MaybeUninit<K>; B],  // separator keys; len valid entries
    children: [NodeId; B + 1],  // child pointers; len + 1 valid entries
}
```

- `B` is a compile-time const generic (default `15` for 8-byte keys → fits in ~256 bytes).
- Keys and children are stored inline — no heap allocation per node.

### 6.3 LeafNode

```rust
struct LeafNode<K, V> {
    len: u16,
    prev: Option<NodeId>,
    next: Option<NodeId>,
    keys: [MaybeUninit<K>; B],
    vals: [MaybeUninit<V>; B],
}
```

- Leaf nodes are doubly linked for bidirectional range scans.
- `vals` are stored inline; for large `V` types the user can store `Box<V>` or use a value ID.

### 6.4 Node Enum

```rust
enum Node<K, V> {
    Internal(InternalNode<K>),
    Leaf(LeafNode<K, V>),
}
```

### 6.5 NodeArena

```rust
struct NodeArena<K, V> {
    nodes: Vec<Node<K, V>>,
    free: Vec<NodeId>,  // free list for deleted nodes
}
```

O(1) allocation and deallocation. The free list means slot reuse is trivial. The `Vec` grows amortized; capacity can be pre-allocated with `with_capacity`.

---

## 7. Algorithms

### 7.1 Search — `get(key) -> Option<&V>`

1. Start at root.
2. In each internal node: binary search `keys[0..len]` to find the correct child pointer.
3. Recurse until a leaf node is reached.
4. Binary search the leaf's keys; return the value if found.

**Complexity:** `O(log_B n)` comparisons, `O(log_B n)` cache misses in the worst case.

### 7.2 Insert — `insert(key, value) -> Option<V>`

Uses **eager splitting** (split-on-the-way-down, also called proactive splitting):

1. If root is full, split root first (increases tree height by 1).
2. Walk down the tree, splitting any full node before descending into it.
3. This guarantees the parent always has room to absorb a new key from a child split.
4. At the leaf, insert the key-value pair in sorted order.
5. Return the old value if the key already existed.

Eager splitting avoids backtracking and requires only a single root-to-leaf pass.

### 7.3 Delete — `remove(key) -> Option<V>`

Uses **lazy merging** (merge/rebalance on the way up):

1. Walk down to the target leaf.
2. Remove the key-value pair.
3. On the way back up, if a node has fewer than `⌈B/2⌉` keys:
   a. Try to **borrow** a key from a sibling (rotate).
   b. If the sibling is at minimum occupancy, **merge** with it and remove the separator from the parent.
4. Handle root shrinkage (if root becomes empty, its only child becomes the new root).

### 7.4 Range Scan — `range(lo, hi) -> Cursor`

1. Search for `lo` to reach the start leaf (same as point lookup path).
2. Return a `Cursor` holding the current `NodeId` and index.
3. The cursor advances by walking `next` pointers across leaf nodes.
4. The cursor terminates when the current key exceeds `hi`.

Range scans are O(k) in the number of returned elements after an O(log n) seek.

### 7.5 Bulk Load — `from_sorted_iter(iter)`

For bulk construction from a sorted iterator:

1. Fill leaf nodes left-to-right, emitting a separator key each time a leaf is full.
2. Build internal node levels bottom-up from the separator keys.
3. No splits occur; all nodes are filled to capacity (or 2/3 fill for headroom).

Bulk load is O(n) and produces a denser, more cache-friendly tree than repeated insertions.

---

## 8. Optimizations

### 8.1 Cache-Line Alignment

Nodes are annotated `#[repr(C, align(64))]` to align to 64-byte cache line boundaries. This prevents false sharing and ensures a node read does not span two cache lines unnecessarily.

### 8.2 Branch Factor Tuning via Const Generics

`BPlusTree<K, V, const B: usize>` allows the caller to tune `B` at compile time. The default is `B = 15`, which fits a leaf node in ~256 bytes (4 cache lines) for 8-byte keys and values. Users with different key/value sizes can override the const parameter.

### 8.3 Binary Search with Hints

Standard binary search is used within nodes. For sequential or nearly-sorted workloads, a linear scan from the last-accessed position (finger search) will be offered as an optional hint on `Cursor`.

### 8.4 Inlining Hot Paths

The search and binary-search-within-node functions will be `#[inline(always)]` to avoid function call overhead on the hot path.

### 8.5 Optimistic Lock Coupling (OLC) — Concurrency

For thread-safe access, we will implement **Optimistic Lock Coupling (OLC)**:

- Each node carries a `version: AtomicU64` (odd = locked, even = unlocked).
- **Reads** proceed without acquiring a lock: snapshot the version, read, re-check the version. Retry if it changed.
- **Writes** acquire an exclusive lock (spin on version becoming even, then CAS to odd).
- **Lock coupling**: during traversal, the parent's lock is released as soon as the child's lock is acquired and the child is verified safe.

This allows reads to proceed entirely without locks in the uncontended case, which is the common case for lookups.

> v1 ships with a `RwLock<BPlusTree>` wrapper for simplicity. OLC is targeted for v2 once correctness is proven.

### 8.6 Small Key Optimization

When `K: Copy + PartialOrd` and `sizeof(K) <= 8`, keys can be compared without memory indirection. The binary search loop in internal nodes is explicitly written to keep keys in registers.

---

## 9. Public API

```rust
// Construction
BPlusTree::new() -> Self
BPlusTree::with_capacity(n: usize) -> Self
BPlusTree::from_sorted_iter(iter: impl Iterator<Item=(K,V)>) -> Result<Self, UnsortedError>

// Point operations — Q allows e.g. get(&"str") on a BPlusTree<String, _>
fn insert(&mut self, key: K, value: V) -> Option<V>
fn remove<Q>(&mut self, key: &Q) -> Option<V>       where K: Borrow<Q>, Q: Ord + ?Sized
fn get<Q>(&self, key: &Q) -> Option<&V>             where K: Borrow<Q>, Q: Ord + ?Sized
fn get_mut<Q>(&mut self, key: &Q) -> Option<&mut V> where K: Borrow<Q>, Q: Ord + ?Sized
fn contains_key<Q>(&self, key: &Q) -> bool          where K: Borrow<Q>, Q: Ord + ?Sized

// Range operations
fn range<Q, R>(&self, range: R) -> Cursor<'_, K, V>
    where K: Borrow<Q>, Q: Ord + ?Sized, R: RangeBounds<Q>

// Bulk
fn extend_sorted(iter: impl Iterator<Item=(K,V)>)  // incremental bulk insert

// Metadata
fn len(&self) -> usize
fn is_empty(&self) -> bool
fn height(&self) -> usize
fn stats(&self) -> TreeStats   // node count, fill factor, etc.

// Standard traits
impl<K: Ord + Clone, V> FromIterator<(K, V)> for BPlusTree<K, V>
impl<K: Ord + Clone, V> IntoIterator for BPlusTree<K, V>
impl<K: Ord + Clone + Debug, V: Debug> Debug for BPlusTree<K, V>
```

### Cursor (Range Iterator)

```rust
struct Cursor<'a, K, V> { /* opaque */ }

impl<'a, K: Ord, V> Iterator for Cursor<'a, K, V> {
    type Item = (&'a K, &'a V);
}
impl<'a, K: Ord, V> DoubleEndedIterator for Cursor<'a, K, V> {}
```

---

## 10. Key Type Constraints

```text
K: Ord + Clone
V: no constraint (sized)
```

`Clone` is needed when promoting a separator key into an internal node during a split. If the user's key type is expensive to clone (e.g., `String`), they should consider `Arc<str>` or integer IDs.

All lookup operations (`get`, `remove`, `contains_key`, `range`) are generic over a borrowed form `Q` via `K: Borrow<Q>, Q: Ord + ?Sized`. This mirrors the `std::collections::BTreeMap` API and lets callers avoid allocating owned keys just to do a lookup — e.g., `tree.get("hello")` works on a `BPlusTree<String, _>`.

---

## 11. Error Handling

The tree operates on in-memory data with well-defined invariants, so most operations are infallible. The only fallible case:

- `from_sorted_iter` validates that the input is actually sorted and returns `Result` if not (debug builds also assert within).

Panics are reserved for invariant violations (bugs); they are documented and should never occur on correct usage.

---

## 12. Testing Strategy

### 12.1 Unit Tests

- Each operation in isolation with small, manually verified inputs.
- Corner cases: empty tree, single element, root splits, root merges, min/max boundary keys.

### 12.2 Invariant Checker

A `check_invariants(&self)` method walks the entire tree and asserts:

1. All leaf nodes reachable from the root.
2. Every internal node has `len+1` children.
3. Keys in every node are strictly sorted.
4. The leaf linked list visits every leaf exactly once.
5. The `len` field of the tree equals the number of unique keys in the leaves.

This is called after every operation in debug/test builds.

### 12.3 Property-Based Tests (`proptest`)

- Random sequences of inserts, deletes, and lookups.
- The B+Tree result must match `BTreeMap<K, V>` (the reference oracle).
- Range queries verified against the same range on `BTreeMap`.

### 12.4 Concurrency Tests (`loom`)

- Once OLC is implemented: model-checked concurrent insert/lookup/delete pairs using `loom`.

### 12.5 Benchmarks (`criterion`)

| Benchmark | Description |
| --------- | ----------- |
| `insert_sequential` | Insert 1M keys in sorted order |
| `insert_random` | Insert 1M keys randomly |
| `lookup_hit` | 1M lookups, all keys present |
| `lookup_miss` | 1M lookups, no keys present |
| `delete_random` | Delete 500K random keys from 1M key tree |
| `range_scan_small` | 10-element range scan on 1M key tree |
| `range_scan_large` | 10K-element range scan on 1M key tree |
| `bulk_load` | Build 1M key tree from sorted iterator |

Compared against `std::collections::BTreeMap` as baseline.

---

## 13. Implementation Phases

### Phase 1 — Core (MVP)

- [ ] `NodeArena` with slab allocation
- [ ] `InternalNode` and `LeafNode` structs
- [ ] Single-threaded `BPlusTree`: `insert`, `get`, `remove`
- [ ] Invariant checker
- [ ] Property-based tests against `BTreeMap` oracle

### Phase 2 — Iterators & Ergonomics

- [ ] `Cursor` / range scan with `Borrow<Q>` range bounds
- [ ] `IntoIterator`, `FromIterator`
- [ ] `Debug` impl
- [ ] `TreeStats` / `stats()`

### Phase 3 — Bulk & Optimization

- [ ] `from_sorted_iter` bulk loader
- [ ] Default `B = 15`; const generic override documented
- [ ] `#[repr(C, align(64))]` on nodes
- [ ] Benchmark suite with criterion

### Phase 4 — Concurrency

- [ ] `RwLock<BPlusTree>` wrapper (safe, simple)
- [ ] Loom-based concurrency tests
- [ ] OLC per-node locking (stretch goal)

---

## 14. File Layout (Proposed)

```text
src/
  lib.rs           — public API re-exports, crate docs
  tree.rs          — BPlusTree<K,V,B> struct and core methods
  node.rs          — Node, InternalNode, LeafNode definitions
  arena.rs         — NodeArena slab allocator
  cursor.rs        — Cursor / range iterator
  bulk.rs          — bulk loading logic
  stats.rs         — TreeStats
  invariants.rs    — debug invariant checker (cfg(test) + pub(crate))
tests/
  correctness.rs   — property-based tests
  concurrent.rs    — loom tests (phase 4)
benches/
  throughput.rs    — criterion benchmarks
```

---

## 15. Decisions

| Decision | Choice | Rationale |
| -------- | ------ | --------- |
| Default branch factor | `B = 15` (fixed) | Fits leaf node in ~256 bytes for 8-byte K+V; dynamic computation from `sizeof` not worth the complexity in stable Rust const generics |
| Key borrowing | `get<Q> where K: Borrow<Q>` from day one | Matches `std::collections::BTreeMap`; avoids forcing callers to allocate owned keys for lookups |
| no-std support | Deferred | Low priority for v1; architecture (arena over raw alloc) is compatible if needed later |
| Value storage | Inline in leaf nodes | Keeps locality; users with large `V` can wrap in `Box<V>` themselves. `SlottedLeaf` variant deferred. |
| Prefix compression | Deferred | Only worthwhile for `String`/byte-slice keys; adds significant implementation complexity for v1 |

---

## 16. References

- Graefe, G. (2011). *Modern B-Tree Techniques*. Foundations and Trends in Databases.
- Leis et al. (2016). *The ART of Practical Synchronization* (OLC description).
- Rust `std::collections::BTreeMap` source — reference for safe Rust B-Tree patterns.
- `beef` / `indexmap` crates — examples of ergonomic Rust collection APIs.
