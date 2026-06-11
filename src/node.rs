#![allow(dead_code)] // write-path methods consumed by issues #4 (insert) and #5 (delete)

use std::borrow::Borrow;
use std::mem::MaybeUninit;

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct NodeId(pub u32);

pub enum Node<K, V, const B: usize>
where
    [(); B + 1]:,
{
    Internal(InternalNode<K, B>),
    Leaf(LeafNode<K, V, B>),
}

pub struct InternalNode<K, const B: usize>
where
    [(); B + 1]:,
{
    pub len: u16,
    pub keys: [MaybeUninit<K>; B],
    pub children: [NodeId; B + 1],
}

pub struct LeafNode<K, V, const B: usize> {
    pub len: u16,
    pub prev: Option<NodeId>,
    pub next: Option<NodeId>,
    pub keys: [MaybeUninit<K>; B],
    pub vals: [MaybeUninit<V>; B],
}

impl<K, const B: usize> InternalNode<K, B>
where
    [(); B + 1]:,
{
    pub fn is_full(&self) -> bool {
        self.len == B as u16
    }

    /// Binary search the separator keys; returns the index of the child to descend into.
    pub fn find_child_idx<Q>(&self, key: &Q) -> usize
    where
        K: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        let mut lo = 0usize;
        let mut hi = self.len as usize;
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            // SAFETY: mid < self.len, so keys[mid] is an initialized separator.
            let sep = unsafe { self.keys[mid].assume_init_ref() };
            if sep.borrow() <= key {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        lo
    }

    /// Insert a separator key and its right child at `idx`, shifting later entries right.
    pub fn insert_key_child_at(&mut self, idx: usize, key: K, child: NodeId) {
        debug_assert!(!self.is_full());
        let len = self.len as usize;
        for i in (idx..len).rev() {
            // SAFETY: i < len <= B-1 (node not full), so keys[i] is initialized and keys[i+1]
            // is a valid slot we overwrite.
            unsafe {
                let k = self.keys[i].assume_init_read();
                self.keys[i + 1].write(k);
            }
        }
        for i in (idx + 1..=len).rev() {
            self.children[i + 1] = self.children[i];
        }
        self.keys[idx].write(key);
        self.children[idx + 1] = child;
        self.len += 1;
    }
}

impl<K, V, const B: usize> LeafNode<K, V, B> {
    pub fn is_full(&self) -> bool {
        self.len == B as u16
    }

    /// Binary search the keys; `Ok(i)` if present at `i`, `Err(i)` for the insertion position.
    pub fn find_key_idx<Q>(&self, key: &Q) -> Result<usize, usize>
    where
        K: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        let mut lo = 0usize;
        let mut hi = self.len as usize;
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            // SAFETY: mid < self.len, so keys[mid] is an initialized key.
            let k = unsafe { self.keys[mid].assume_init_ref() };
            match k.borrow().cmp(key) {
                std::cmp::Ordering::Less => lo = mid + 1,
                std::cmp::Ordering::Greater => hi = mid,
                std::cmp::Ordering::Equal => return Ok(mid),
            }
        }
        Err(lo)
    }

    /// Insert a key-value pair at `idx`, shifting later entries right.
    pub fn insert_at(&mut self, idx: usize, key: K, val: V) {
        debug_assert!(!self.is_full());
        let len = self.len as usize;
        for i in (idx..len).rev() {
            // SAFETY: i < len <= B-1 (node not full), so slots i are initialized and slots
            // i+1 are valid targets within the arrays.
            unsafe {
                let k = self.keys[i].assume_init_read();
                self.keys[i + 1].write(k);
                let v = self.vals[i].assume_init_read();
                self.vals[i + 1].write(v);
            }
        }
        self.keys[idx].write(key);
        self.vals[idx].write(val);
        self.len += 1;
    }

    /// Remove the pair at `idx`, shifting later entries left, and return it.
    pub fn remove_at(&mut self, idx: usize) -> (K, V) {
        let len = self.len as usize;
        debug_assert!(idx < len);
        // SAFETY: idx < len, so both slots are initialized.
        let key = unsafe { self.keys[idx].assume_init_read() };
        // SAFETY: idx < len, so both slots are initialized.
        let val = unsafe { self.vals[idx].assume_init_read() };
        for i in idx + 1..len {
            // SAFETY: i < len, so slot i is initialized; slot i-1 was just vacated.
            unsafe {
                let k = self.keys[i].assume_init_read();
                self.keys[i - 1].write(k);
                let v = self.vals[i].assume_init_read();
                self.vals[i - 1].write(v);
            }
        }
        self.len -= 1;
        (key, val)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const B: usize = 4;

    fn empty_internal() -> InternalNode<i32, B> {
        InternalNode {
            len: 0,
            keys: [const { MaybeUninit::uninit() }; B],
            children: [NodeId(0); B + 1],
        }
    }

    fn empty_leaf() -> LeafNode<i32, i32, B> {
        LeafNode {
            len: 0,
            prev: None,
            next: None,
            keys: [const { MaybeUninit::uninit() }; B],
            vals: [const { MaybeUninit::uninit() }; B],
        }
    }

    #[test]
    fn internal_empty_find() {
        let node = empty_internal();
        assert_eq!(node.find_child_idx(&5), 0);
    }

    #[test]
    fn internal_find_child_bounds() {
        let mut node = empty_internal();
        node.insert_key_child_at(0, 10, NodeId(1));
        node.insert_key_child_at(1, 20, NodeId(2));
        node.insert_key_child_at(2, 30, NodeId(3));
        assert_eq!(node.find_child_idx(&5), 0);
        assert_eq!(node.find_child_idx(&10), 1);
        assert_eq!(node.find_child_idx(&15), 1);
        assert_eq!(node.find_child_idx(&25), 2);
        assert_eq!(node.find_child_idx(&30), 3);
        assert_eq!(node.find_child_idx(&99), 3);
    }

    #[test]
    fn internal_insert_at_head_tail_middle() {
        let mut node = empty_internal();
        node.insert_key_child_at(0, 10, NodeId(1));
        node.insert_key_child_at(1, 30, NodeId(3));
        node.insert_key_child_at(1, 20, NodeId(2));
        assert_eq!(node.len, 3);
        let keys: Vec<i32> = (0..node.len as usize)
            // SAFETY: indices < len are initialized.
            .map(|i| unsafe { *node.keys[i].assume_init_ref() })
            .collect();
        assert_eq!(keys, vec![10, 20, 30]);
        assert_eq!(node.children[0], NodeId(0));
        assert_eq!(node.children[1], NodeId(1));
        assert_eq!(node.children[2], NodeId(2));
        assert_eq!(node.children[3], NodeId(3));
    }

    #[test]
    fn internal_is_full() {
        let mut node = empty_internal();
        assert!(!node.is_full());
        for i in 0..B {
            node.insert_key_child_at(i, i as i32, NodeId(i as u32 + 1));
        }
        assert!(node.is_full());
    }

    #[test]
    #[should_panic]
    fn internal_insert_when_full_panics() {
        let mut node = empty_internal();
        for i in 0..B {
            node.insert_key_child_at(i, i as i32, NodeId(i as u32 + 1));
        }
        node.insert_key_child_at(0, 99, NodeId(99));
    }

    #[test]
    fn leaf_empty_find() {
        let leaf = empty_leaf();
        assert_eq!(leaf.find_key_idx(&5), Err(0));
    }

    #[test]
    fn leaf_single_element() {
        let mut leaf = empty_leaf();
        leaf.insert_at(0, 10, 100);
        assert_eq!(leaf.find_key_idx(&10), Ok(0));
        assert_eq!(leaf.find_key_idx(&5), Err(0));
        assert_eq!(leaf.find_key_idx(&15), Err(1));
    }

    #[test]
    fn leaf_insert_at_head_tail_middle() {
        let mut leaf = empty_leaf();
        leaf.insert_at(0, 10, 100);
        leaf.insert_at(1, 30, 300);
        leaf.insert_at(1, 20, 200);
        assert_eq!(leaf.len, 3);
        assert_eq!(leaf.find_key_idx(&10), Ok(0));
        assert_eq!(leaf.find_key_idx(&20), Ok(1));
        assert_eq!(leaf.find_key_idx(&30), Ok(2));
        let vals: Vec<i32> = (0..leaf.len as usize)
            // SAFETY: indices < len are initialized.
            .map(|i| unsafe { *leaf.vals[i].assume_init_ref() })
            .collect();
        assert_eq!(vals, vec![100, 200, 300]);
    }

    #[test]
    fn leaf_remove_at_head_tail_middle() {
        let mut leaf = empty_leaf();
        for i in 0..4 {
            leaf.insert_at(i, (i as i32 + 1) * 10, (i as i32 + 1) * 100);
        }
        assert_eq!(leaf.remove_at(0), (10, 100));
        assert_eq!(leaf.len, 3);
        assert_eq!(leaf.remove_at(2), (40, 400));
        assert_eq!(leaf.len, 2);
        assert_eq!(leaf.remove_at(1), (30, 300));
        assert_eq!(leaf.len, 1);
        assert_eq!(leaf.find_key_idx(&20), Ok(0));
    }

    #[test]
    fn leaf_is_full() {
        let mut leaf = empty_leaf();
        assert!(!leaf.is_full());
        for i in 0..B {
            leaf.insert_at(i, i as i32, i as i32);
        }
        assert!(leaf.is_full());
    }

    #[test]
    #[should_panic]
    fn leaf_insert_when_full_panics() {
        let mut leaf = empty_leaf();
        for i in 0..B {
            leaf.insert_at(i, i as i32, i as i32);
        }
        leaf.insert_at(0, 99, 99);
    }

    #[test]
    fn leaf_borrowed_lookup() {
        let mut leaf: LeafNode<String, i32, B> = LeafNode {
            len: 0,
            prev: None,
            next: None,
            keys: [const { MaybeUninit::uninit() }; B],
            vals: [const { MaybeUninit::uninit() }; B],
        };
        leaf.insert_at(0, "apple".to_string(), 1);
        leaf.insert_at(1, "banana".to_string(), 2);
        assert_eq!(leaf.find_key_idx("apple"), Ok(0));
        assert_eq!(leaf.find_key_idx("cherry"), Err(2));
    }
}
