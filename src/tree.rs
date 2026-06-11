use std::borrow::Borrow;
use std::mem::MaybeUninit;

use crate::arena::NodeArena;
use crate::node::{Node, NodeId};

/// An in-memory B+Tree mapping keys of type `K` to values of type `V`.
pub struct BPlusTree<K, V, const B: usize = 15>
where
    [(); B + 1]:,
{
    arena: NodeArena<K, V, B>,
    root: NodeId,
    len: usize,
}

impl<K, V, const B: usize> BPlusTree<K, V, B>
where
    K: Ord,
    [(); B + 1]:,
{
    /// Creates an empty tree whose root is a single empty leaf.
    pub fn new() -> Self {
        let mut arena = NodeArena::new();
        let root = arena.alloc_leaf();
        BPlusTree {
            arena,
            root,
            len: 0,
        }
    }

    /// Creates an empty tree, pre-allocating arena capacity for `n` nodes.
    pub fn with_capacity(n: usize) -> Self {
        let mut arena = NodeArena::with_capacity(n);
        let root = arena.alloc_leaf();
        BPlusTree {
            arena,
            root,
            len: 0,
        }
    }

    /// Returns the number of key-value pairs in the tree.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Returns `true` if the tree contains no entries.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Returns the height of the tree; a root-only tree has height 1.
    pub fn height(&self) -> usize {
        let mut height = 1;
        let mut id = self.root;
        while let Node::Internal(internal) = self.arena.get(id) {
            id = internal.children[0];
            height += 1;
        }
        height
    }

    /// Returns a reference to the value for `key`, or `None` if absent.
    pub fn get<Q>(&self, key: &Q) -> Option<&V>
    where
        K: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        let leaf = self.arena.get_leaf(self.find_leaf(key));
        match leaf.find_key_idx(key) {
            // SAFETY: find_key_idx returns Ok(idx) only for idx < len, so the slot is initialized.
            Ok(idx) => Some(unsafe { leaf.vals[idx].assume_init_ref() }),
            Err(_) => None,
        }
    }

    /// Returns `true` if the tree contains `key`.
    pub fn contains_key<Q>(&self, key: &Q) -> bool
    where
        K: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        let leaf = self.arena.get_leaf(self.find_leaf(key));
        leaf.find_key_idx(key).is_ok()
    }

    fn find_leaf<Q>(&self, key: &Q) -> NodeId
    where
        K: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        let mut id = self.root;
        while let Node::Internal(internal) = self.arena.get(id) {
            id = internal.children[internal.find_child_idx(key)];
        }
        id
    }
}

impl<K, V, const B: usize> BPlusTree<K, V, B>
where
    K: Ord + Clone,
    [(); B + 1]:,
{
    /// Inserts `key`/`value`, returning the previous value if `key` was already present.
    pub fn insert(&mut self, key: K, value: V) -> Option<V> {
        if self.node_is_full(self.root) {
            self.split_root();
        }

        let mut id = self.root;
        loop {
            match self.arena.get(id) {
                Node::Internal(internal) => {
                    let child_idx = internal.find_child_idx(&key);
                    let child_id = internal.children[child_idx];
                    if self.node_is_full(child_id) {
                        self.split_child(id, child_idx);
                        let internal = self.arena.get_internal(id);
                        // SAFETY: split_child wrote a separator at `child_idx`; it is initialized.
                        let sep = unsafe { internal.keys[child_idx].assume_init_ref() };
                        id = if key < *sep {
                            internal.children[child_idx]
                        } else {
                            internal.children[child_idx + 1]
                        };
                    } else {
                        id = child_id;
                    }
                }
                Node::Leaf(_) => {
                    let leaf = self.arena.get_leaf_mut(id);
                    match leaf.find_key_idx(&key) {
                        Ok(idx) => {
                            // SAFETY: idx < len, so the value slot is initialized; we overwrite it.
                            let old = unsafe { leaf.vals[idx].assume_init_mut() };
                            return Some(std::mem::replace(old, value));
                        }
                        Err(idx) => {
                            leaf.insert_at(idx, key, value);
                            self.len += 1;
                            return None;
                        }
                    }
                }
            }
        }
    }

    fn node_is_full(&self, id: NodeId) -> bool {
        match self.arena.get(id) {
            Node::Internal(internal) => internal.is_full(),
            Node::Leaf(leaf) => leaf.is_full(),
        }
    }

    /// Splits the full root, creating a new internal root one level taller.
    fn split_root(&mut self) {
        let old_root = self.root;
        let new_root = self.arena.alloc_internal();
        self.arena.get_internal_mut(new_root).children[0] = old_root;
        self.root = new_root;
        self.split_child(new_root, 0);
    }

    /// Splits the full child at `parent.children[child_idx]`, promoting its median into `parent`.
    fn split_child(&mut self, parent_id: NodeId, child_idx: usize) {
        debug_assert!(self.node_is_full(self.arena.get_internal(parent_id).children[child_idx]));
        let mid = B / 2;
        let child_id = self.arena.get_internal(parent_id).children[child_idx];

        let (separator, new_right) = match self.arena.get(child_id) {
            Node::Internal(_) => self.split_internal(child_id, mid),
            Node::Leaf(_) => self.split_leaf(child_id, mid),
        };

        self.arena
            .get_internal_mut(parent_id)
            .insert_key_child_at(child_idx, separator, new_right);
    }

    fn split_internal(&mut self, child_id: NodeId, mid: usize) -> (K, NodeId) {
        let new_id = self.arena.alloc_internal();

        let child = self.arena.get_internal_mut(child_id);
        // SAFETY: child is full, so keys[mid] is initialized; it is moved out and child.len
        // shrinks below mid so it is never read or dropped again from the left node.
        let separator = unsafe { child.keys[mid].assume_init_read() };

        let mut right_keys: [MaybeUninit<K>; B] = [const { MaybeUninit::uninit() }; B];
        let mut right_children = [NodeId(0); B + 1];
        let right_len = (B - mid - 1) as u16;
        for (i, slot) in right_keys[..right_len as usize].iter_mut().enumerate() {
            // SAFETY: mid+1+i < B (child was full), so the source key slot is initialized; it is
            // moved into the right node and child.len shrinks past it.
            slot.write(unsafe { child.keys[mid + 1 + i].assume_init_read() });
        }
        right_children[..=right_len as usize].copy_from_slice(&child.children[mid + 1..=B]);
        child.len = mid as u16;

        let right = self.arena.get_internal_mut(new_id);
        right.keys = right_keys;
        right.children = right_children;
        right.len = right_len;

        (separator, new_id)
    }

    fn split_leaf(&mut self, child_id: NodeId, mid: usize) -> (K, NodeId) {
        let new_id = self.arena.alloc_leaf();

        let child = self.arena.get_leaf_mut(child_id);
        let right_len = (B - mid) as u16;
        let old_next = child.next;

        let mut right_keys: [MaybeUninit<K>; B] = [const { MaybeUninit::uninit() }; B];
        let mut right_vals: [MaybeUninit<V>; B] = [const { MaybeUninit::uninit() }; B];
        for i in 0..right_len as usize {
            // SAFETY: mid+i < B (child was full), so source slots are initialized; they are moved
            // into the right node and child.len shrinks to mid so they are not touched again.
            right_keys[i].write(unsafe { child.keys[mid + i].assume_init_read() });
            right_vals[i].write(unsafe { child.vals[mid + i].assume_init_read() });
        }
        child.len = mid as u16;
        child.next = Some(new_id);

        // right_keys[0] holds the moved old keys[mid]. A leaf split pushes up a *copy* of the
        // first right key while the key itself stays in the right leaf, so we clone here.
        // SAFETY: right_len >= 1 (mid < B), so right_keys[0] was just written and is initialized.
        let separator = unsafe { right_keys[0].assume_init_ref().clone() };

        let right = self.arena.get_leaf_mut(new_id);
        right.keys = right_keys;
        right.vals = right_vals;
        right.len = right_len;
        right.prev = Some(child_id);
        right.next = old_next;

        if let Some(next_id) = old_next {
            self.arena.get_leaf_mut(next_id).prev = Some(new_id);
        }

        (separator, new_id)
    }
}

impl<K, V, const B: usize> Default for BPlusTree<K, V, B>
where
    K: Ord,
    [(); B + 1]:,
{
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const B: usize = 4;

    #[test]
    fn new_tree_is_empty() {
        let tree: BPlusTree<i32, i32, B> = BPlusTree::new();
        assert_eq!(tree.len(), 0);
        assert!(tree.is_empty());
    }

    #[test]
    fn height_of_fresh_tree_is_one() {
        let tree: BPlusTree<i32, i32, B> = BPlusTree::new();
        assert_eq!(tree.height(), 1);
    }

    #[test]
    fn get_on_empty_tree_misses() {
        let tree: BPlusTree<i32, i32, B> = BPlusTree::new();
        assert_eq!(tree.get(&42), None);
    }

    #[test]
    fn get_hit_and_miss() {
        let mut tree: BPlusTree<i32, i32, B> = BPlusTree::new();
        let leaf = tree.arena.get_leaf_mut(tree.root);
        leaf.insert_at(0, 10, 100);
        leaf.insert_at(1, 20, 200);
        tree.len = 2;
        assert_eq!(tree.get(&10), Some(&100));
        assert_eq!(tree.get(&20), Some(&200));
        assert_eq!(tree.get(&15), None);
    }

    #[test]
    fn contains_key_hit_and_miss() {
        let mut tree: BPlusTree<i32, i32, B> = BPlusTree::new();
        let leaf = tree.arena.get_leaf_mut(tree.root);
        leaf.insert_at(0, 10, 100);
        tree.len = 1;
        assert!(tree.contains_key(&10));
        assert!(!tree.contains_key(&99));
    }

    #[test]
    fn get_with_borrowed_key() {
        let mut tree: BPlusTree<String, u32, B> = BPlusTree::new();
        let leaf = tree.arena.get_leaf_mut(tree.root);
        leaf.insert_at(0, "apple".to_string(), 1);
        leaf.insert_at(1, "banana".to_string(), 2);
        tree.len = 2;
        assert_eq!(tree.get("apple"), Some(&1));
        assert_eq!(tree.get("banana"), Some(&2));
        assert_eq!(tree.get("cherry"), None);
        assert!(tree.contains_key("apple"));
        assert!(!tree.contains_key("cherry"));
    }

    fn collect_leaves<const BB: usize>(tree: &BPlusTree<i32, i32, BB>) -> Vec<(i32, i32)>
    where
        [(); BB + 1]:,
    {
        let mut id = tree.root;
        while let Node::Internal(internal) = tree.arena.get(id) {
            id = internal.children[0];
        }
        let mut out = Vec::new();
        let mut prev: Option<NodeId> = None;
        loop {
            let leaf = tree.arena.get_leaf(id);
            assert_eq!(leaf.prev, prev, "prev back-link is broken");
            for i in 0..leaf.len as usize {
                // SAFETY: i < len, slots initialized.
                let k = unsafe { *leaf.keys[i].assume_init_ref() };
                let v = unsafe { *leaf.vals[i].assume_init_ref() };
                out.push((k, v));
            }
            match leaf.next {
                Some(next) => {
                    prev = Some(id);
                    id = next;
                }
                None => break,
            }
        }
        out
    }

    #[test]
    fn insert_into_empty_tree() {
        let mut tree: BPlusTree<i32, i32, B> = BPlusTree::new();
        assert_eq!(tree.insert(10, 100), None);
        assert_eq!(tree.len(), 1);
        assert_eq!(tree.get(&10), Some(&100));
        assert_eq!(tree.height(), 1);
    }

    #[test]
    fn duplicate_insert_returns_old_value() {
        let mut tree: BPlusTree<i32, i32, B> = BPlusTree::new();
        assert_eq!(tree.insert(10, 100), None);
        assert_eq!(tree.insert(10, 999), Some(100));
        assert_eq!(tree.len(), 1);
        assert_eq!(tree.get(&10), Some(&999));
    }

    #[test]
    fn sequential_inserts_force_splits() {
        let mut tree: BPlusTree<i32, i32, B> = BPlusTree::new();
        let n = 100;
        for i in 0..n {
            assert_eq!(tree.insert(i, i * 10), None);
        }
        assert_eq!(tree.len(), n as usize);
        assert!(tree.height() > 1, "height should grow past 1 after splits");
        for i in 0..n {
            assert_eq!(tree.get(&i), Some(&(i * 10)));
        }
        let leaves = collect_leaves(&tree);
        let expected: Vec<(i32, i32)> = (0..n).map(|i| (i, i * 10)).collect();
        assert_eq!(leaves, expected);
    }

    #[test]
    fn reverse_inserts_force_splits() {
        let mut tree: BPlusTree<i32, i32, B> = BPlusTree::new();
        for i in (0..50).rev() {
            assert_eq!(tree.insert(i, i * 10), None);
        }
        assert_eq!(tree.len(), 50);
        let leaves = collect_leaves(&tree);
        let expected: Vec<(i32, i32)> = (0..50).map(|i| (i, i * 10)).collect();
        assert_eq!(leaves, expected);
    }

    #[test]
    fn random_inserts_match_btreemap() {
        use std::collections::BTreeMap;
        let mut tree: BPlusTree<i32, i32, B> = BPlusTree::new();
        let mut oracle = BTreeMap::new();
        let mut state: u64 = 0x1234_5678;
        for _ in 0..500 {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let key = (state >> 33) as i32 % 200;
            let val = key * 7;
            assert_eq!(tree.insert(key, val), oracle.insert(key, val));
        }
        assert_eq!(tree.len(), oracle.len());
        for (&k, &v) in &oracle {
            assert_eq!(tree.get(&k), Some(&v));
        }
        let leaves = collect_leaves(&tree);
        let expected: Vec<(i32, i32)> = oracle.into_iter().collect();
        assert_eq!(leaves, expected);
    }

    #[test]
    fn root_split_increases_height() {
        let mut tree: BPlusTree<i32, i32, B> = BPlusTree::new();
        for i in 0..B as i32 {
            tree.insert(i, i);
        }
        assert_eq!(tree.height(), 1);
        tree.insert(B as i32, B as i32);
        assert_eq!(tree.height(), 2);
    }
}
