//! Job-owned, generation-checked storage for mutable TeX nodes.
//!
//! A [`NodeId`] is meaningful only in the arena that created it. Removing a
//! node increments its slot generation before the slot can be reused, so stale
//! and cross-job handles fail deterministically instead of addressing another
//! node. Lists own links; allocated but unlinked nodes have no predecessor,
//! successor, or owner list.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::boxes::{Node, NodeList, PackResult, HBOX};

static NEXT_ARENA_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId {
    arena: u64,
    slot: u32,
    generation: u32,
}

impl NodeId {
    pub const fn arena(self) -> u64 {
        self.arena
    }

    pub const fn slot(self) -> u32 {
        self.slot
    }

    pub const fn generation(self) -> u32 {
        self.generation
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeListId {
    arena: u64,
    slot: u32,
    generation: u32,
}

impl NodeListId {
    pub const fn arena(self) -> u64 {
        self.arena
    }

    pub const fn slot(self) -> u32 {
        self.slot
    }

    pub const fn generation(self) -> u32 {
        self.generation
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum NodeProperty {
    Boolean(bool),
    Integer(i64),
    Number(f64),
    Bytes(Vec<u8>),
    Node(NodeId),
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct NodeMetadata {
    attributes: BTreeMap<i32, i32>,
    properties: crate::FxHashMap<Vec<u8>, NodeProperty>,
}

impl NodeMetadata {
    pub fn attribute(&self, key: i32) -> Option<i32> {
        self.attributes.get(&key).copied()
    }

    pub fn set_attribute(&mut self, key: i32, value: Option<i32>) {
        if let Some(value) = value {
            self.attributes.insert(key, value);
        } else {
            self.attributes.remove(&key);
        }
    }

    pub fn property(&self, key: &[u8]) -> Option<&NodeProperty> {
        self.properties.get(key)
    }

    pub fn set_property(&mut self, key: Vec<u8>, value: Option<NodeProperty>) {
        if let Some(value) = value {
            self.properties.insert(key, value);
        } else {
            self.properties.remove(key.as_slice());
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NodeLinks {
    pub list: Option<NodeListId>,
    pub prev: Option<NodeId>,
    pub next: Option<NodeId>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NodeListLinks {
    pub head: Option<NodeId>,
    pub tail: Option<NodeId>,
    pub len: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NodeArenaError {
    WrongArena,
    StaleNode,
    StaleList,
    AlreadyLinked,
    NotLinked,
    WrongList,
    ListNotEmpty,
    CorruptLinks,
    CapacityExceeded,
}

impl fmt::Display for NodeArenaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::WrongArena => "node handle belongs to another job",
            Self::StaleNode => "node handle is stale",
            Self::StaleList => "node-list handle is stale",
            Self::AlreadyLinked => "node is already linked",
            Self::NotLinked => "node is not linked",
            Self::WrongList => "node belongs to another list",
            Self::ListNotEmpty => "node list is not empty",
            Self::CorruptLinks => "node-list links are inconsistent",
            Self::CapacityExceeded => "node arena capacity exceeded",
        };
        f.write_str(message)
    }
}

impl std::error::Error for NodeArenaError {}

#[derive(Clone, Debug)]
struct NodeSlot {
    generation: u32,
    value: Option<Node>,
    metadata: NodeMetadata,
    list: Option<NodeListId>,
    prev: Option<NodeId>,
    next: Option<NodeId>,
}

impl NodeSlot {
    fn vacant(generation: u32) -> Self {
        Self {
            generation,
            value: None,
            metadata: NodeMetadata::default(),
            list: None,
            prev: None,
            next: None,
        }
    }
}

#[derive(Clone, Debug)]
struct ListSlot {
    generation: u32,
    alive: bool,
    head: Option<NodeId>,
    tail: Option<NodeId>,
    len: usize,
}

impl ListSlot {
    fn vacant(generation: u32) -> Self {
        Self {
            generation,
            alive: false,
            head: None,
            tail: None,
            len: 0,
        }
    }
}

#[derive(Debug)]
pub struct NodeArena {
    id: u64,
    nodes: Vec<NodeSlot>,
    free_nodes: Vec<u32>,
    lists: Vec<ListSlot>,
    free_lists: Vec<u32>,
}

impl Default for NodeArena {
    fn default() -> Self {
        Self::new()
    }
}

impl NodeArena {
    pub fn new() -> Self {
        let mut id = NEXT_ARENA_ID.fetch_add(1, Ordering::Relaxed);
        if id == 0 {
            id = NEXT_ARENA_ID.fetch_add(1, Ordering::Relaxed);
        }
        Self {
            id,
            nodes: Vec::new(),
            free_nodes: Vec::new(),
            lists: Vec::new(),
            free_lists: Vec::new(),
        }
    }

    pub const fn id(&self) -> u64 {
        self.id
    }

    pub fn allocated_nodes(&self) -> usize {
        self.nodes
            .iter()
            .filter(|slot| slot.value.is_some())
            .count()
    }

    pub fn allocated_lists(&self) -> usize {
        self.lists.iter().filter(|slot| slot.alive).count()
    }

    pub fn alloc(&mut self, value: Node) -> Result<NodeId, NodeArenaError> {
        let slot = if let Some(slot) = self.free_nodes.pop() {
            slot
        } else {
            let slot =
                u32::try_from(self.nodes.len()).map_err(|_| NodeArenaError::CapacityExceeded)?;
            self.nodes.push(NodeSlot::vacant(1));
            slot
        };
        let entry = &mut self.nodes[slot as usize];
        debug_assert!(entry.value.is_none());
        entry.value = Some(value);
        entry.metadata = NodeMetadata::default();
        entry.list = None;
        entry.prev = None;
        entry.next = None;
        Ok(NodeId {
            arena: self.id,
            slot,
            generation: entry.generation,
        })
    }

    pub fn new_list(&mut self) -> Result<NodeListId, NodeArenaError> {
        let slot = if let Some(slot) = self.free_lists.pop() {
            slot
        } else {
            let slot =
                u32::try_from(self.lists.len()).map_err(|_| NodeArenaError::CapacityExceeded)?;
            self.lists.push(ListSlot::vacant(1));
            slot
        };
        let entry = &mut self.lists[slot as usize];
        debug_assert!(!entry.alive);
        entry.alive = true;
        entry.head = None;
        entry.tail = None;
        entry.len = 0;
        Ok(NodeListId {
            arena: self.id,
            slot,
            generation: entry.generation,
        })
    }

    fn node_index(&self, id: NodeId) -> Result<usize, NodeArenaError> {
        if id.arena != self.id {
            return Err(NodeArenaError::WrongArena);
        }
        let index = id.slot as usize;
        let Some(slot) = self.nodes.get(index) else {
            return Err(NodeArenaError::StaleNode);
        };
        if slot.generation != id.generation || slot.value.is_none() {
            return Err(NodeArenaError::StaleNode);
        }
        Ok(index)
    }

    fn list_index(&self, id: NodeListId) -> Result<usize, NodeArenaError> {
        if id.arena != self.id {
            return Err(NodeArenaError::WrongArena);
        }
        let index = id.slot as usize;
        let Some(slot) = self.lists.get(index) else {
            return Err(NodeArenaError::StaleList);
        };
        if slot.generation != id.generation || !slot.alive {
            return Err(NodeArenaError::StaleList);
        }
        Ok(index)
    }

    pub fn contains(&self, id: NodeId) -> bool {
        self.node_index(id).is_ok()
    }

    pub fn contains_list(&self, id: NodeListId) -> bool {
        self.list_index(id).is_ok()
    }

    pub fn get(&self, id: NodeId) -> Result<&Node, NodeArenaError> {
        let index = self.node_index(id)?;
        self.nodes[index]
            .value
            .as_ref()
            .ok_or(NodeArenaError::StaleNode)
    }

    pub fn get_mut(&mut self, id: NodeId) -> Result<&mut Node, NodeArenaError> {
        let index = self.node_index(id)?;
        self.nodes[index]
            .value
            .as_mut()
            .ok_or(NodeArenaError::StaleNode)
    }

    pub fn metadata(&self, id: NodeId) -> Result<&NodeMetadata, NodeArenaError> {
        let index = self.node_index(id)?;
        Ok(&self.nodes[index].metadata)
    }

    pub fn metadata_mut(&mut self, id: NodeId) -> Result<&mut NodeMetadata, NodeArenaError> {
        let index = self.node_index(id)?;
        Ok(&mut self.nodes[index].metadata)
    }

    pub fn links(&self, id: NodeId) -> Result<NodeLinks, NodeArenaError> {
        let index = self.node_index(id)?;
        let slot = &self.nodes[index];
        Ok(NodeLinks {
            list: slot.list,
            prev: slot.prev,
            next: slot.next,
        })
    }

    pub fn list_links(&self, id: NodeListId) -> Result<NodeListLinks, NodeArenaError> {
        let index = self.list_index(id)?;
        let slot = &self.lists[index];
        Ok(NodeListLinks {
            head: slot.head,
            tail: slot.tail,
            len: slot.len,
        })
    }

    pub fn push_back(&mut self, list: NodeListId, node: NodeId) -> Result<(), NodeArenaError> {
        let list_index = self.list_index(list)?;
        let node_index = self.node_index(node)?;
        if self.nodes[node_index].list.is_some() {
            return Err(NodeArenaError::AlreadyLinked);
        }
        let old_tail = self.lists[list_index].tail;
        if let Some(tail) = old_tail {
            let tail_index = self.node_index(tail)?;
            self.nodes[tail_index].next = Some(node);
        } else {
            self.lists[list_index].head = Some(node);
        }
        let node_slot = &mut self.nodes[node_index];
        node_slot.list = Some(list);
        node_slot.prev = old_tail;
        node_slot.next = None;
        self.lists[list_index].tail = Some(node);
        self.lists[list_index].len += 1;
        Ok(())
    }

    pub fn push_front(&mut self, list: NodeListId, node: NodeId) -> Result<(), NodeArenaError> {
        let list_index = self.list_index(list)?;
        let node_index = self.node_index(node)?;
        if self.nodes[node_index].list.is_some() {
            return Err(NodeArenaError::AlreadyLinked);
        }
        let old_head = self.lists[list_index].head;
        if let Some(head) = old_head {
            let head_index = self.node_index(head)?;
            self.nodes[head_index].prev = Some(node);
        } else {
            self.lists[list_index].tail = Some(node);
        }
        let node_slot = &mut self.nodes[node_index];
        node_slot.list = Some(list);
        node_slot.prev = None;
        node_slot.next = old_head;
        self.lists[list_index].head = Some(node);
        self.lists[list_index].len += 1;
        Ok(())
    }

    pub fn insert_before(&mut self, anchor: NodeId, node: NodeId) -> Result<(), NodeArenaError> {
        let anchor_index = self.node_index(anchor)?;
        let node_index = self.node_index(node)?;
        if self.nodes[node_index].list.is_some() {
            return Err(NodeArenaError::AlreadyLinked);
        }
        let list = self.nodes[anchor_index]
            .list
            .ok_or(NodeArenaError::NotLinked)?;
        let list_index = self.list_index(list)?;
        let prev = self.nodes[anchor_index].prev;
        if let Some(prev) = prev {
            let prev_index = self.node_index(prev)?;
            self.nodes[prev_index].next = Some(node);
        } else {
            self.lists[list_index].head = Some(node);
        }
        self.nodes[anchor_index].prev = Some(node);
        let node_slot = &mut self.nodes[node_index];
        node_slot.list = Some(list);
        node_slot.prev = prev;
        node_slot.next = Some(anchor);
        self.lists[list_index].len += 1;
        Ok(())
    }

    pub fn insert_after(&mut self, anchor: NodeId, node: NodeId) -> Result<(), NodeArenaError> {
        let anchor_index = self.node_index(anchor)?;
        let node_index = self.node_index(node)?;
        if self.nodes[node_index].list.is_some() {
            return Err(NodeArenaError::AlreadyLinked);
        }
        let list = self.nodes[anchor_index]
            .list
            .ok_or(NodeArenaError::NotLinked)?;
        let list_index = self.list_index(list)?;
        let next = self.nodes[anchor_index].next;
        if let Some(next) = next {
            let next_index = self.node_index(next)?;
            self.nodes[next_index].prev = Some(node);
        } else {
            self.lists[list_index].tail = Some(node);
        }
        self.nodes[anchor_index].next = Some(node);
        let node_slot = &mut self.nodes[node_index];
        node_slot.list = Some(list);
        node_slot.prev = Some(anchor);
        node_slot.next = next;
        self.lists[list_index].len += 1;
        Ok(())
    }

    pub fn unlink(&mut self, node: NodeId) -> Result<(), NodeArenaError> {
        let node_index = self.node_index(node)?;
        let list = self.nodes[node_index]
            .list
            .ok_or(NodeArenaError::NotLinked)?;
        let list_index = self.list_index(list)?;
        let prev = self.nodes[node_index].prev;
        let next = self.nodes[node_index].next;
        if let Some(prev) = prev {
            let prev_index = self.node_index(prev)?;
            if self.nodes[prev_index].next != Some(node) {
                return Err(NodeArenaError::CorruptLinks);
            }
            self.nodes[prev_index].next = next;
        } else if self.lists[list_index].head == Some(node) {
            self.lists[list_index].head = next;
        } else {
            return Err(NodeArenaError::CorruptLinks);
        }
        if let Some(next) = next {
            let next_index = self.node_index(next)?;
            if self.nodes[next_index].prev != Some(node) {
                return Err(NodeArenaError::CorruptLinks);
            }
            self.nodes[next_index].prev = prev;
        } else if self.lists[list_index].tail == Some(node) {
            self.lists[list_index].tail = prev;
        } else {
            return Err(NodeArenaError::CorruptLinks);
        }
        let node_slot = &mut self.nodes[node_index];
        node_slot.list = None;
        node_slot.prev = None;
        node_slot.next = None;
        self.lists[list_index].len = self.lists[list_index]
            .len
            .checked_sub(1)
            .ok_or(NodeArenaError::CorruptLinks)?;
        Ok(())
    }

    pub fn remove(&mut self, node: NodeId) -> Result<Node, NodeArenaError> {
        if self.links(node)?.list.is_some() {
            self.unlink(node)?;
        }
        self.free_unlinked(node)
    }

    pub fn free_unlinked(&mut self, node: NodeId) -> Result<Node, NodeArenaError> {
        let index = self.node_index(node)?;
        if self.nodes[index].list.is_some() {
            return Err(NodeArenaError::AlreadyLinked);
        }
        let slot = &mut self.nodes[index];
        let value = slot.value.take().ok_or(NodeArenaError::StaleNode)?;
        slot.metadata = NodeMetadata::default();
        slot.prev = None;
        slot.next = None;
        slot.generation = next_generation(slot.generation);
        self.free_nodes.push(node.slot);
        Ok(value)
    }

    pub fn pop_front(&mut self, list: NodeListId) -> Result<Option<Node>, NodeArenaError> {
        let head = self.list_links(list)?.head;
        head.map(|node| self.remove(node)).transpose()
    }

    pub fn pop_back(&mut self, list: NodeListId) -> Result<Option<Node>, NodeArenaError> {
        let tail = self.list_links(list)?.tail;
        tail.map(|node| self.remove(node)).transpose()
    }

    pub fn remove_list(&mut self, list: NodeListId) -> Result<(), NodeArenaError> {
        let index = self.list_index(list)?;
        if self.lists[index].len != 0 {
            return Err(NodeArenaError::ListNotEmpty);
        }
        let slot = &mut self.lists[index];
        slot.alive = false;
        slot.head = None;
        slot.tail = None;
        slot.generation = next_generation(slot.generation);
        self.free_lists.push(list.slot);
        Ok(())
    }

    pub fn node_ids(&self, list: NodeListId) -> Result<Vec<NodeId>, NodeArenaError> {
        let links = self.list_links(list)?;
        let mut result = Vec::with_capacity(links.len);
        let mut current = links.head;
        while let Some(node) = current {
            if result.len() >= links.len {
                return Err(NodeArenaError::CorruptLinks);
            }
            let node_links = self.links(node)?;
            if node_links.list != Some(list) {
                return Err(NodeArenaError::CorruptLinks);
            }
            result.push(node);
            current = node_links.next;
        }
        if result.len() != links.len || result.last().copied() != links.tail {
            return Err(NodeArenaError::CorruptLinks);
        }
        Ok(result)
    }

    pub fn alloc_list<I>(&mut self, nodes: I) -> Result<NodeListId, NodeArenaError>
    where
        I: IntoIterator<Item = Node>,
    {
        let list = self.new_list()?;
        for node in nodes {
            let node = self.alloc(node)?;
            self.push_back(list, node)?;
        }
        Ok(list)
    }

    pub fn take_list(&mut self, list: NodeListId) -> Result<NodeList, NodeArenaError> {
        let ids = self.node_ids(list)?;
        let mut nodes = Vec::with_capacity(ids.len());
        for id in ids {
            nodes.push(self.remove(id)?);
        }
        self.remove_list(list)?;
        Ok(nodes)
    }

    pub fn copy_node(&mut self, node: NodeId) -> Result<NodeId, NodeArenaError> {
        let index = self.node_index(node)?;
        let value = self.nodes[index]
            .value
            .as_ref()
            .ok_or(NodeArenaError::StaleNode)?
            .clone();
        let metadata = self.nodes[index].metadata.clone();
        let copy = self.alloc(value)?;
        let copy_index = self.node_index(copy)?;
        self.nodes[copy_index].metadata = metadata;
        Ok(copy)
    }

    pub fn copy_list(&mut self, list: NodeListId) -> Result<NodeListId, NodeArenaError> {
        let ids = self.node_ids(list)?;
        let copy = self.new_list()?;
        for id in ids {
            let node = self.copy_node(id)?;
            self.push_back(copy, node)?;
        }
        Ok(copy)
    }

    pub fn pack_hlist(
        &mut self,
        list: NodeListId,
        target: Option<i32>,
        eqtb: &crate::eqtb::Eqtb,
    ) -> Result<NodeId, NodeArenaError> {
        let nodes = self.take_list(list)?;
        let result: PackResult = crate::boxes::hpack(nodes, target, HBOX, eqtb);
        self.alloc(result.node)
    }
}

fn next_generation(generation: u32) -> u32 {
    let next = generation.wrapping_add(1);
    if next == 0 {
        1
    } else {
        next
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::boxes::{Glue, Node};

    #[test]
    fn stale_and_cross_arena_handles_are_rejected_after_slot_reuse() {
        let mut arena = NodeArena::new();
        let old = arena.alloc(Node::Penalty(1)).unwrap();
        assert!(matches!(arena.remove(old), Ok(Node::Penalty(1))));
        assert_eq!(arena.get(old).unwrap_err(), NodeArenaError::StaleNode);

        let replacement = arena.alloc(Node::Penalty(2)).unwrap();
        assert_eq!(replacement.slot(), old.slot());
        assert_ne!(replacement.generation(), old.generation());
        assert!(matches!(arena.get(replacement), Ok(Node::Penalty(2))));

        let mut other = NodeArena::new();
        assert_eq!(
            other.get(replacement).unwrap_err(),
            NodeArenaError::WrongArena
        );
        let other_node = other.alloc(Node::Penalty(3)).unwrap();
        assert_eq!(
            arena.get(other_node).unwrap_err(),
            NodeArenaError::WrongArena
        );
    }

    #[test]
    fn insertion_unlink_and_removal_preserve_head_tail_and_links() {
        let mut arena = NodeArena::new();
        let list = arena.new_list().unwrap();
        let one = arena.alloc(Node::Penalty(1)).unwrap();
        let two = arena.alloc(Node::Penalty(2)).unwrap();
        let three = arena.alloc(Node::Penalty(3)).unwrap();
        arena.push_back(list, one).unwrap();
        arena.insert_after(one, three).unwrap();
        arena.insert_before(three, two).unwrap();
        assert_eq!(arena.node_ids(list).unwrap(), vec![one, two, three]);
        assert_eq!(
            arena.list_links(list).unwrap(),
            NodeListLinks {
                head: Some(one),
                tail: Some(three),
                len: 3,
            }
        );

        arena.unlink(two).unwrap();
        assert_eq!(arena.node_ids(list).unwrap(), vec![one, three]);
        assert_eq!(arena.links(two).unwrap().list, None);
        arena.push_front(list, two).unwrap();
        assert_eq!(arena.node_ids(list).unwrap(), vec![two, one, three]);
        assert!(matches!(arena.remove(one), Ok(Node::Penalty(1))));
        assert_eq!(arena.node_ids(list).unwrap(), vec![two, three]);
    }

    #[test]
    fn copied_nodes_and_child_lists_are_independent() {
        let mut arena = NodeArena::new();
        let child = vec![Node::Kern(7), Node::Glue(Glue::new(3))];
        let original = arena
            .alloc(Node::Box {
                kind: HBOX,
                w: 10,
                h: 2,
                d: 1,
                shift: 0,
                list: child,
                glue_sign: 0,
                glue_order: 0,
                glue_set: 0.0,
                font: None,
            })
            .unwrap();
        arena
            .metadata_mut(original)
            .unwrap()
            .set_attribute(4, Some(9));
        let copy = arena.copy_node(original).unwrap();
        assert_eq!(arena.metadata(copy).unwrap().attribute(4), Some(9));

        let Node::Box { list, .. } = arena.get_mut(copy).unwrap() else {
            panic!("copied node is not a box");
        };
        list.push(Node::Penalty(20));
        let Node::Box { list, .. } = arena.get(original).unwrap() else {
            panic!("original node is not a box");
        };
        assert_eq!(list.len(), 2);
    }

    #[test]
    fn arena_list_can_drive_real_hpack_operation() {
        let mut arena = NodeArena::new();
        let list = arena
            .alloc_list([Node::Kern(100), Node::Glue(Glue::new(25))])
            .unwrap();
        let eqtb = crate::eqtb::Eqtb::new(true);
        let packed = arena.pack_hlist(list, None, &eqtb).unwrap();
        let Node::Box {
            w,
            list: packed_list,
            ..
        } = arena.get(packed).unwrap()
        else {
            panic!("hpack did not create a box");
        };
        assert_eq!(*w, 125);
        assert_eq!(packed_list.len(), 2);
        assert_eq!(
            arena.list_links(list).unwrap_err(),
            NodeArenaError::StaleList
        );
    }
}
