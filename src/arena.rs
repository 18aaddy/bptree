#![allow(dead_code)] // alloc/free and typed accessors consumed by issues #4 and #5

use std::mem::MaybeUninit;

use crate::node::{InternalNode, LeafNode, Node, NodeId};

pub struct NodeArena<K, V, const B: usize>
where
    [(); B + 1]:,
{
    nodes: Vec<Node<K, V, B>>,
    free: Vec<NodeId>,
}

impl<K, V, const B: usize> NodeArena<K, V, B>
where
    [(); B + 1]:,
{
    pub fn new() -> Self {
        NodeArena {
            nodes: Vec::new(),
            free: Vec::new(),
        }
    }

    pub fn with_capacity(n: usize) -> Self {
        NodeArena {
            nodes: Vec::with_capacity(n),
            free: Vec::with_capacity(n),
        }
    }

    pub fn alloc_leaf(&mut self) -> NodeId {
        let leaf = Node::Leaf(LeafNode {
            len: 0,
            prev: None,
            next: None,
            keys: [const { MaybeUninit::uninit() }; B],
            vals: [const { MaybeUninit::uninit() }; B],
        });
        self.alloc(leaf)
    }

    pub fn alloc_internal(&mut self) -> NodeId {
        let internal = Node::Internal(InternalNode {
            len: 0,
            keys: [const { MaybeUninit::uninit() }; B],
            children: [NodeId(0); B + 1],
        });
        self.alloc(internal)
    }

    fn alloc(&mut self, node: Node<K, V, B>) -> NodeId {
        if let Some(id) = self.free.pop() {
            self.nodes[id.0 as usize] = node;
            id
        } else {
            let id = NodeId(self.nodes.len() as u32);
            self.nodes.push(node);
            id
        }
    }

    /// Drops the node's initialized contents and returns its slot to the free list.
    pub fn free(&mut self, id: NodeId) {
        debug_assert!(!self.free.contains(&id), "double free of NodeId({})", id.0);
        match &mut self.nodes[id.0 as usize] {
            Node::Internal(node) => {
                for slot in node.keys[..node.len as usize].iter_mut() {
                    // SAFETY: slots below len are initialized; len is set to 0 after so they
                    // are never dropped again.
                    unsafe { slot.assume_init_drop() };
                }
                node.len = 0;
            }
            Node::Leaf(node) => {
                for slot in node.keys[..node.len as usize].iter_mut() {
                    // SAFETY: slots below len are initialized; len is set to 0 after so they
                    // are never dropped again.
                    unsafe { slot.assume_init_drop() };
                }
                for slot in node.vals[..node.len as usize].iter_mut() {
                    // SAFETY: slots below len are initialized; len is set to 0 after so they
                    // are never dropped again.
                    unsafe { slot.assume_init_drop() };
                }
                node.len = 0;
            }
        }
        self.free.push(id);
    }

    pub fn get(&self, id: NodeId) -> &Node<K, V, B> {
        &self.nodes[id.0 as usize]
    }

    pub fn get_mut(&mut self, id: NodeId) -> &mut Node<K, V, B> {
        &mut self.nodes[id.0 as usize]
    }

    pub fn get_leaf(&self, id: NodeId) -> &LeafNode<K, V, B> {
        match self.get(id) {
            Node::Leaf(leaf) => leaf,
            Node::Internal(_) => panic!("NodeId({}) is Internal, expected Leaf", id.0),
        }
    }

    pub fn get_leaf_mut(&mut self, id: NodeId) -> &mut LeafNode<K, V, B> {
        match self.get_mut(id) {
            Node::Leaf(leaf) => leaf,
            Node::Internal(_) => panic!("NodeId({}) is Internal, expected Leaf", id.0),
        }
    }

    pub fn get_internal(&self, id: NodeId) -> &InternalNode<K, B> {
        match self.get(id) {
            Node::Internal(internal) => internal,
            Node::Leaf(_) => panic!("NodeId({}) is Leaf, expected Internal", id.0),
        }
    }

    pub fn get_internal_mut(&mut self, id: NodeId) -> &mut InternalNode<K, B> {
        match self.get_mut(id) {
            Node::Internal(internal) => internal,
            Node::Leaf(_) => panic!("NodeId({}) is Leaf, expected Internal", id.0),
        }
    }
}

impl<K, V, const B: usize> Default for NodeArena<K, V, B>
where
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
    fn alloc_leaf_and_internal() {
        let mut arena: NodeArena<i32, i32, B> = NodeArena::new();
        let leaf = arena.alloc_leaf();
        let internal = arena.alloc_internal();
        assert_eq!(leaf, NodeId(0));
        assert_eq!(internal, NodeId(1));
        assert!(matches!(arena.get(leaf), Node::Leaf(_)));
        assert!(matches!(arena.get(internal), Node::Internal(_)));
    }

    #[test]
    fn free_reuses_slot() {
        let mut arena: NodeArena<i32, i32, B> = NodeArena::new();
        let a = arena.alloc_leaf();
        let b = arena.alloc_leaf();
        arena.free(a);
        let c = arena.alloc_internal();
        assert_eq!(c, a);
        assert_eq!(b, NodeId(1));
        assert!(matches!(arena.get(c), Node::Internal(_)));
    }

    #[test]
    fn with_capacity_allocates() {
        let mut arena: NodeArena<i32, i32, B> = NodeArena::with_capacity(8);
        let id = arena.alloc_leaf();
        assert_eq!(id, NodeId(0));
    }

    #[test]
    fn typed_accessor_happy_path() {
        let mut arena: NodeArena<i32, i32, B> = NodeArena::new();
        let leaf = arena.alloc_leaf();
        let internal = arena.alloc_internal();
        assert_eq!(arena.get_leaf(leaf).len, 0);
        assert_eq!(arena.get_internal(internal).len, 0);
        arena.get_leaf_mut(leaf).len = 2;
        assert_eq!(arena.get_leaf(leaf).len, 2);
    }

    #[test]
    #[should_panic(expected = "expected Leaf")]
    fn get_leaf_on_internal_panics() {
        let mut arena: NodeArena<i32, i32, B> = NodeArena::new();
        let internal = arena.alloc_internal();
        arena.get_leaf(internal);
    }

    #[test]
    #[should_panic(expected = "expected Internal")]
    fn get_internal_on_leaf_panics() {
        let mut arena: NodeArena<i32, i32, B> = NodeArena::new();
        let leaf = arena.alloc_leaf();
        arena.get_internal(leaf);
    }

    #[test]
    #[should_panic(expected = "double free")]
    fn double_free_panics() {
        let mut arena: NodeArena<i32, i32, B> = NodeArena::new();
        let a = arena.alloc_leaf();
        arena.free(a);
        arena.free(a);
    }

    #[test]
    fn free_drops_initialized_values() {
        use std::rc::Rc;
        let mut arena: NodeArena<i32, Rc<()>, B> = NodeArena::new();
        let id = arena.alloc_leaf();
        let witness = Rc::new(());
        let leaf = arena.get_leaf_mut(id);
        leaf.insert_at(0, 10, Rc::clone(&witness));
        leaf.insert_at(1, 20, Rc::clone(&witness));
        assert_eq!(Rc::strong_count(&witness), 3);
        arena.free(id);
        assert_eq!(Rc::strong_count(&witness), 1);
    }
}
