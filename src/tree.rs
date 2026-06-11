use std::borrow::Borrow;

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
}
