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

    /// Removes `key`, returning its value if present.
    pub fn remove<Q>(&mut self, key: &Q) -> Option<V>
    where
        K: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        let (removed, _) = self.remove_recursive(self.root, key);
        if removed.is_some() {
            self.len -= 1;
            self.maybe_collapse_root();
        }
        removed
    }

    fn remove_recursive<Q>(&mut self, node_id: NodeId, key: &Q) -> (Option<V>, bool)
    where
        K: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        match self.arena.get(node_id) {
            Node::Leaf(leaf) => match leaf.find_key_idx(key) {
                Err(_) => (None, false),
                Ok(idx) => {
                    let leaf = self.arena.get_leaf_mut(node_id);
                    let (_, val) = leaf.remove_at(idx);
                    (Some(val), self.underflows(node_id))
                }
            },
            Node::Internal(internal) => {
                let child_idx = internal.find_child_idx(key);
                let child_id = internal.children[child_idx];
                let (removed, child_underflowed) = self.remove_recursive(child_id, key);
                if child_underflowed {
                    self.fix_underflow(node_id, child_idx);
                }
                (removed, self.underflows(node_id))
            }
        }
    }

    fn underflows(&self, id: NodeId) -> bool {
        let min = (B - 1) / 2;
        match self.arena.get(id) {
            Node::Internal(internal) => (internal.len as usize) < min,
            Node::Leaf(leaf) => (leaf.len as usize) < min,
        }
    }

    fn can_lend(&self, id: NodeId) -> bool {
        let min = (B - 1) / 2;
        match self.arena.get(id) {
            Node::Internal(internal) => (internal.len as usize) > min,
            Node::Leaf(leaf) => (leaf.len as usize) > min,
        }
    }

    fn fix_underflow(&mut self, parent_id: NodeId, child_idx: usize) {
        let parent = self.arena.get_internal(parent_id);
        let has_left = child_idx > 0;
        let has_right = child_idx < parent.len as usize;

        if has_left && self.can_lend(parent.children[child_idx - 1]) {
            self.rotate_right(parent_id, child_idx);
        } else if has_right && self.can_lend(parent.children[child_idx + 1]) {
            self.rotate_left(parent_id, child_idx);
        } else if has_left {
            self.merge_children(parent_id, child_idx - 1);
        } else {
            self.merge_children(parent_id, child_idx);
        }
    }

    fn rotate_right(&mut self, parent_id: NodeId, child_idx: usize) {
        let parent = self.arena.get_internal(parent_id);
        let left_id = parent.children[child_idx - 1];
        let child_id = parent.children[child_idx];
        let sep_idx = child_idx - 1;

        match self.arena.get(child_id) {
            Node::Leaf(_) => {
                let left = self.arena.get_leaf_mut(left_id);
                let (k, v) = left.remove_at(left.len as usize - 1);
                let child = self.arena.get_leaf_mut(child_id);
                child.insert_at(0, k.clone(), v);
                let parent = self.arena.get_internal_mut(parent_id);
                // SAFETY: sep_idx < parent.len, so the separator slot is initialized; overwritten.
                unsafe { parent.keys[sep_idx].assume_init_drop() };
                parent.keys[sep_idx].write(k);
            }
            Node::Internal(_) => {
                let parent = self.arena.get_internal_mut(parent_id);
                // SAFETY: sep_idx < parent.len, so the separator is initialized; moved down.
                let sep = unsafe { parent.keys[sep_idx].assume_init_read() };
                let left = self.arena.get_internal_mut(left_id);
                let last = left.len as usize - 1;
                // SAFETY: last < left.len, so the key is initialized; moved up to the parent.
                let lent_key = unsafe { left.keys[last].assume_init_read() };
                let lent_child = left.children[last + 1];
                left.len -= 1;
                let child = self.arena.get_internal_mut(child_id);
                child.insert_front(sep, lent_child);
                let parent = self.arena.get_internal_mut(parent_id);
                parent.keys[sep_idx].write(lent_key);
            }
        }
    }

    fn rotate_left(&mut self, parent_id: NodeId, child_idx: usize) {
        let parent = self.arena.get_internal(parent_id);
        let right_id = parent.children[child_idx + 1];
        let child_id = parent.children[child_idx];
        let sep_idx = child_idx;

        match self.arena.get(child_id) {
            Node::Leaf(_) => {
                let right = self.arena.get_leaf_mut(right_id);
                let (k, v) = right.remove_at(0);
                // SAFETY: right still has >= 1 key (it lent from a node above min); slot 0 initialized.
                let new_sep = unsafe { right.keys[0].assume_init_ref().clone() };
                let child = self.arena.get_leaf_mut(child_id);
                child.insert_at(child.len as usize, k, v);
                let parent = self.arena.get_internal_mut(parent_id);
                // SAFETY: sep_idx < parent.len, so the separator slot is initialized; overwritten.
                unsafe { parent.keys[sep_idx].assume_init_drop() };
                parent.keys[sep_idx].write(new_sep);
            }
            Node::Internal(_) => {
                let parent = self.arena.get_internal_mut(parent_id);
                // SAFETY: sep_idx < parent.len, so the separator is initialized; moved down.
                let sep = unsafe { parent.keys[sep_idx].assume_init_read() };
                let right = self.arena.get_internal_mut(right_id);
                let (lent_key, lent_child) = right.pop_front();
                let child = self.arena.get_internal_mut(child_id);
                child.push_back(sep, lent_child);
                let parent = self.arena.get_internal_mut(parent_id);
                parent.keys[sep_idx].write(lent_key);
            }
        }
    }

    fn merge_children(&mut self, parent_id: NodeId, left_idx: usize) {
        let parent = self.arena.get_internal(parent_id);
        let left_id = parent.children[left_idx];
        let right_id = parent.children[left_idx + 1];

        let separator = self
            .arena
            .get_internal_mut(parent_id)
            .pop_separator(left_idx);

        match self.arena.get(left_id) {
            Node::Leaf(_) => {
                // Leaf separators are copies of a leaf key, not pulled into the merged node.
                drop(separator);
                let right = self.arena.get_leaf_mut(right_id);
                let right_next = right.next;
                let moved: Vec<(K, V)> = (0..right.len as usize)
                    // SAFETY: i < right.len, so both slots are initialized; moved out and
                    // right.len is zeroed so free() does not drop them again.
                    .map(|i| unsafe {
                        (
                            right.keys[i].assume_init_read(),
                            right.vals[i].assume_init_read(),
                        )
                    })
                    .collect();
                right.len = 0;
                let left = self.arena.get_leaf_mut(left_id);
                let mut at = left.len as usize;
                for (k, v) in moved {
                    left.insert_at(at, k, v);
                    at += 1;
                }
                left.next = right_next;
                if let Some(next_id) = right_next {
                    self.arena.get_leaf_mut(next_id).prev = Some(left_id);
                }
            }
            Node::Internal(_) => {
                let right = self.arena.get_internal_mut(right_id);
                let right_len = right.len as usize;
                let moved_keys: Vec<K> = (0..right_len)
                    // SAFETY: i < right.len, so keys[i] is initialized; moved out, right.len zeroed.
                    .map(|i| unsafe { right.keys[i].assume_init_read() })
                    .collect();
                let moved_children: Vec<NodeId> = right.children[..=right_len].to_vec();
                right.len = 0;
                let left = self.arena.get_internal_mut(left_id);
                let mut at = left.len as usize;
                left.keys[at].write(separator);
                left.children[at + 1] = moved_children[0];
                left.len += 1;
                at += 1;
                for (i, k) in moved_keys.into_iter().enumerate() {
                    left.keys[at].write(k);
                    left.children[at + 1] = moved_children[i + 1];
                    left.len += 1;
                    at += 1;
                }
            }
        }

        self.arena.free(right_id);
    }

    fn maybe_collapse_root(&mut self) {
        if let Node::Internal(internal) = self.arena.get(self.root) {
            if internal.len == 0 {
                let only_child = internal.children[0];
                let old_root = self.root;
                self.root = only_child;
                self.arena.free(old_root);
            }
        }
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

impl<K, V, const B: usize> Drop for BPlusTree<K, V, B>
where
    [(); B + 1]:,
{
    fn drop(&mut self) {
        let mut stack = vec![self.root];
        while let Some(id) = stack.pop() {
            if let Node::Internal(internal) = self.arena.get(id) {
                stack.extend_from_slice(&internal.children[..=internal.len as usize]);
            }
            self.arena.free(id);
        }
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

    #[test]
    fn remove_non_existent_key() {
        let mut tree: BPlusTree<i32, i32, B> = BPlusTree::new();
        tree.insert(10, 100);
        assert_eq!(tree.remove(&99), None);
        assert_eq!(tree.len(), 1);
        assert_eq!(tree.get(&10), Some(&100));
    }

    #[test]
    fn remove_from_single_leaf() {
        let mut tree: BPlusTree<i32, i32, B> = BPlusTree::new();
        tree.insert(10, 100);
        tree.insert(20, 200);
        assert_eq!(tree.remove(&10), Some(100));
        assert_eq!(tree.len(), 1);
        assert_eq!(tree.get(&10), None);
        assert_eq!(tree.get(&20), Some(&200));
    }

    #[test]
    fn remove_last_element_leaves_empty_root_leaf() {
        let mut tree: BPlusTree<i32, i32, B> = BPlusTree::new();
        tree.insert(42, 1);
        assert_eq!(tree.remove(&42), Some(1));
        assert!(tree.is_empty());
        assert_eq!(tree.len(), 0);
        assert_eq!(tree.height(), 1);
        assert!(matches!(tree.arena.get(tree.root), Node::Leaf(_)));
    }

    #[test]
    fn remove_causing_merge_collapses_root() {
        let mut tree: BPlusTree<i32, i32, B> = BPlusTree::new();
        for i in 0..5 {
            tree.insert(i, i * 10);
        }
        assert_eq!(tree.height(), 2);
        for i in 0..4 {
            assert_eq!(tree.remove(&i), Some(i * 10));
        }
        assert_eq!(tree.height(), 1);
        assert_eq!(tree.len(), 1);
        assert_eq!(tree.get(&4), Some(&40));
    }

    #[test]
    fn remove_preserves_leaf_links_and_order() {
        let mut tree: BPlusTree<i32, i32, B> = BPlusTree::new();
        for i in 0..30 {
            tree.insert(i, i);
        }
        for i in (0..30).step_by(3) {
            assert_eq!(tree.remove(&i), Some(i));
        }
        let leaves = collect_leaves(&tree);
        let expected: Vec<(i32, i32)> = (0..30).filter(|i| i % 3 != 0).map(|i| (i, i)).collect();
        assert_eq!(leaves, expected);
    }

    fn interleaved_oracle<const BB: usize>(seed: u64, ops: usize, modulo: i32)
    where
        [(); BB + 1]:,
    {
        use std::collections::BTreeMap;
        let mut tree: BPlusTree<i32, i32, BB> = BPlusTree::new();
        let mut oracle = BTreeMap::new();
        let mut state = seed;
        for _ in 0..ops {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let key = (state >> 33) as i32 % modulo;
            if state & 1 == 0 {
                assert_eq!(tree.insert(key, key * 7), oracle.insert(key, key * 7));
            } else {
                assert_eq!(tree.remove(&key), oracle.remove(&key));
            }
            assert_eq!(tree.len(), oracle.len());
        }
        let leaves: Vec<(i32, i32)> = {
            let mut id = tree.root;
            while let Node::Internal(internal) = tree.arena.get(id) {
                id = internal.children[0];
            }
            let mut out = Vec::new();
            let mut prev: Option<NodeId> = None;
            loop {
                let leaf = tree.arena.get_leaf(id);
                assert_eq!(leaf.prev, prev, "prev back-link broken");
                for i in 0..leaf.len as usize {
                    // SAFETY: i < len, slots initialized.
                    out.push(unsafe {
                        (
                            *leaf.keys[i].assume_init_ref(),
                            *leaf.vals[i].assume_init_ref(),
                        )
                    });
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
        };
        let expected: Vec<(i32, i32)> = oracle.iter().map(|(&k, &v)| (k, v)).collect();
        assert_eq!(leaves, expected);
        for (&k, &v) in &oracle {
            assert_eq!(tree.get(&k), Some(&v));
        }
    }

    #[test]
    fn interleaved_insert_remove_match_btreemap_b4() {
        interleaved_oracle::<4>(0xDEAD_BEEF, 2000, 60);
    }

    #[test]
    fn interleaved_insert_remove_match_btreemap_b5() {
        interleaved_oracle::<5>(0x0BADF00D, 2000, 80);
    }

    #[test]
    fn interleaved_insert_remove_match_btreemap_b15() {
        interleaved_oracle::<15>(0xFACEFEED, 5000, 300);
    }

    #[test]
    fn string_keys_force_internal_merge() {
        let mut tree: BPlusTree<String, u32, 4> = BPlusTree::new();
        for i in 0..40u32 {
            tree.insert(format!("k{i:03}"), i);
        }
        for i in 0..40u32 {
            assert_eq!(tree.remove(format!("k{i:03}").as_str()), Some(i));
            assert_eq!(tree.len(), (39 - i) as usize);
        }
        assert!(tree.is_empty());
    }

    #[test]
    fn string_keys_force_rotation() {
        let mut tree: BPlusTree<String, u32, 4> = BPlusTree::new();
        for i in 0..40u32 {
            tree.insert(format!("k{i:03}"), i);
        }
        // Delete from the high end so left siblings keep spare keys -> right rotation.
        for i in (10..40u32).rev() {
            assert_eq!(tree.remove(format!("k{i:03}").as_str()), Some(i));
        }
        // Delete from the low end -> left rotation from right siblings.
        for i in 0..10u32 {
            assert_eq!(tree.remove(format!("k{i:03}").as_str()), Some(i));
        }
        assert!(tree.is_empty());
    }

    #[test]
    fn interleaved_string_keys_no_double_free() {
        use std::collections::BTreeMap;
        let mut tree: BPlusTree<String, u64, 4> = BPlusTree::new();
        let mut oracle = BTreeMap::new();
        let mut state: u64 = 0xC0FFEE;
        for _ in 0..3000 {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let key = format!("k{:04}", (state >> 33) % 80);
            if state & 1 == 0 {
                assert_eq!(tree.insert(key.clone(), state), oracle.insert(key, state));
            } else {
                assert_eq!(tree.remove(key.as_str()), oracle.remove(&key));
            }
            assert_eq!(tree.len(), oracle.len());
        }
        for (k, &v) in &oracle {
            assert_eq!(tree.get(k.as_str()), Some(&v));
        }
    }

    #[test]
    fn drop_does_not_leak() {
        use std::rc::Rc;
        let witness = Rc::new(());
        {
            let mut tree: BPlusTree<i32, Rc<()>, B> = BPlusTree::new();
            for i in 0..200 {
                tree.insert(i, Rc::clone(&witness));
            }
            assert_eq!(Rc::strong_count(&witness), 201);
        }
        assert_eq!(Rc::strong_count(&witness), 1);
    }

    #[test]
    fn drop_after_removals_does_not_leak() {
        use std::rc::Rc;
        let witness = Rc::new(());
        {
            let mut tree: BPlusTree<i32, Rc<()>, B> = BPlusTree::new();
            for i in 0..100 {
                tree.insert(i, Rc::clone(&witness));
            }
            for i in 0..50 {
                tree.remove(&i);
            }
            assert_eq!(Rc::strong_count(&witness), 51);
        }
        assert_eq!(Rc::strong_count(&witness), 1);
    }
}
