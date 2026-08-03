//! Pointer-free import and export of Congee's adaptive radix-tree topology.
//!
//! This module exposes a typed interchange representation rather than a byte
//! codec. Durable framing, checksums, generations, and write-ahead logging are
//! intentionally owned by the caller.

use std::fmt;
use std::marker::PhantomData;
use std::ptr::NonNull;
use std::sync::Arc;

use crate::cast_ptr;
use crate::congee_inner::CongeeInner;
use crate::nodes::{BaseNode, NodePtr, NodeType};
use crate::{Allocator, CongeeRaw, DefaultAllocator};

/// Version of the typed topology interchange contract.
pub const VERSION: u16 = 1;

/// An exported, pointer-free Congee topology.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Topology<V> {
    /// Interchange contract version. Must equal [`VERSION`].
    pub version: u16,
    /// Root adaptive node. Congee keeps a root node even when empty.
    pub root: Node<V>,
}

/// A Congee adaptive node.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Node<V> {
    /// Exact adaptive node representation.
    pub kind: NodeKind,
    /// Path-compressed bytes stored in the node header.
    pub prefix: Vec<u8>,
    /// Live branches and their physical child slots.
    pub branches: Vec<Branch<V>>,
    /// Node48 free slots in next-allocation order; empty for other node kinds.
    pub free_slots: Vec<u8>,
}

/// A radix byte, physical slot, and child.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Branch<V> {
    /// Radix byte selecting this branch.
    pub key: u8,
    /// Physical child slot in the recorded node representation.
    pub slot: u16,
    /// Payload or subnode reached by this branch.
    pub child: Child<V>,
}

/// A caller-encoded payload or nested Congee node.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Child<V> {
    /// Caller-encoded logical value; never Congee's raw pointer-sized payload.
    Value(V),
    /// Nested adaptive node.
    Node(Node<V>),
}

/// Congee's adaptive node representations.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum NodeKind {
    /// Up to four branches.
    N4,
    /// Up to sixteen branches.
    N16,
    /// Up to forty-eight indirect child slots.
    N48,
    /// Directly addressed 256-way node.
    N256,
}

impl NodeKind {
    fn capacity(self) -> usize {
        self.into_raw().capacity()
    }

    fn from_raw(kind: NodeType) -> Self {
        match kind {
            NodeType::N4 => Self::N4,
            NodeType::N16 => Self::N16,
            NodeType::N48 => Self::N48,
            NodeType::N256 => Self::N256,
        }
    }

    fn into_raw(self) -> NodeType {
        match self {
            Self::N4 => NodeType::N4,
            Self::N16 => NodeType::N16,
            Self::N48 => NodeType::N48,
            Self::N256 => NodeType::N256,
        }
    }
}

/// A malformed, concurrently changed, or unallocatable topology.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// The interchange version is unsupported.
    UnsupportedVersion {
        /// Version found in the topology.
        found: u16,
    },
    /// The root node contains a prefix, which Congee does not permit.
    RootPrefix,
    /// A compressed path does not fit Congee's fixed eight-byte keys.
    InvalidKeyLength {
        /// Number of bytes represented by the path.
        found: usize,
        /// Required fixed key size.
        expected: usize,
    },
    /// A node contains too many branches for its representation.
    NodeCapacity {
        /// Recorded node representation.
        kind: NodeKind,
        /// Branch count found.
        found: usize,
    },
    /// Two branches in one node use the same radix byte.
    DuplicateKey {
        /// Duplicated radix byte.
        key: u8,
    },
    /// A physical child slot is invalid or duplicated.
    InvalidSlot {
        /// Recorded node representation.
        kind: NodeKind,
        /// Invalid physical slot.
        slot: u16,
    },
    /// Node48's live and free slots do not form an exact partition.
    InvalidFreeList,
    /// A node changed while it was being exported.
    ConcurrentMutation,
    /// The configured Congee allocator could not restore a node.
    OutOfMemory,
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedVersion { found } => {
                write!(formatter, "unsupported Congee topology version {found}")
            }
            Self::RootPrefix => formatter.write_str("Congee root node must not have a prefix"),
            Self::InvalidKeyLength { found, expected } => write!(
                formatter,
                "Congee path has {found} key bytes but requires {expected}",
            ),
            Self::NodeCapacity { kind, found } => {
                write!(formatter, "{kind:?} cannot contain {found} branches")
            }
            Self::DuplicateKey { key } => {
                write!(formatter, "Congee node contains duplicate byte {key}")
            }
            Self::InvalidSlot { kind, slot } => {
                write!(formatter, "invalid or duplicate {kind:?} slot {slot}")
            }
            Self::InvalidFreeList => formatter.write_str("invalid Congee N48 free list"),
            Self::ConcurrentMutation => {
                formatter.write_str("Congee changed during topology export")
            }
            Self::OutOfMemory => formatter.write_str("Congee topology allocation failed"),
        }
    }
}

impl std::error::Error for Error {}

impl<V> Topology<V> {
    /// Validate node kinds, compressed paths, slots, and Node48 free lists.
    pub fn validate(&self) -> Result<(), Error> {
        if self.version != VERSION {
            return Err(Error::UnsupportedVersion {
                found: self.version,
            });
        }
        if !self.root.prefix.is_empty() {
            return Err(Error::RootPrefix);
        }
        validate_node(&self.root, 0)
    }
}

impl<K, V, A> CongeeRaw<K, V, A>
where
    K: Copy + From<usize>,
    V: Copy + From<usize>,
    usize: From<K> + From<V>,
    A: Allocator + Clone + Send,
{
    /// Export an exact pointer-free topology while the tree is exclusively borrowed.
    ///
    /// `encode` must copy or encode the logical value referenced by a raw Congee
    /// payload. Requiring `&mut self` prevents concurrent operations through this
    /// tree and adds no synchronization to point operations.
    pub fn export_topology<T>(
        &mut self,
        mut encode: impl FnMut(V) -> T,
    ) -> Result<Topology<T>, Error> {
        let root = export_node(self.inner.load_root(), &mut |payload| {
            encode(V::from(payload))
        })?;
        let topology = Topology {
            version: VERSION,
            root,
        };
        topology.validate()?;
        Ok(topology)
    }

    /// Restore a topology with a no-op value drainer.
    pub fn from_topology<T>(
        topology: Topology<T>,
        allocator: A,
        decode: impl FnMut(T) -> V,
    ) -> Result<Self, Error> {
        Self::from_topology_with_drainer(topology, allocator, decode, |_key, _value| {})
    }

    /// Restore a topology and install the value drainer used when the tree drops.
    pub fn from_topology_with_drainer<T>(
        topology: Topology<T>,
        allocator: A,
        mut decode: impl FnMut(T) -> V,
        drainer: impl Fn(K, V) + 'static,
    ) -> Result<Self, Error> {
        topology.validate()?;
        let drain_callback: Arc<dyn Fn([u8; 8], usize)> = Arc::new(move |key, value| {
            drainer(K::from(usize::from_be_bytes(key)), V::from(value));
        });

        let root = import_node(topology.root, &allocator, &drain_callback, &mut |value| {
            usize::from(decode(value))
        })?;
        let root = root.into_inner();

        Ok(Self {
            inner: CongeeInner::from_root(root, allocator, drain_callback),
            pt_key: PhantomData,
            pt_val: PhantomData,
        })
    }
}

impl<K, V> CongeeRaw<K, V, DefaultAllocator>
where
    K: Copy + From<usize>,
    V: Copy + From<usize>,
    usize: From<K> + From<V>,
{
    /// Restore using Congee's default allocator and a no-op value drainer.
    pub fn from_topology_default<T>(
        topology: Topology<T>,
        decode: impl FnMut(T) -> V,
    ) -> Result<Self, Error> {
        Self::from_topology(topology, DefaultAllocator {}, decode)
    }
}

fn validate_node<V>(node: &Node<V>, path_bytes: usize) -> Result<(), Error> {
    let path_bytes = path_bytes
        .checked_add(node.prefix.len())
        .ok_or(Error::InvalidKeyLength {
            found: usize::MAX,
            expected: 8,
        })?;
    if path_bytes > 8 {
        return Err(Error::InvalidKeyLength {
            found: path_bytes,
            expected: 8,
        });
    }
    if node.branches.len() > node.kind.capacity() {
        return Err(Error::NodeCapacity {
            kind: node.kind,
            found: node.branches.len(),
        });
    }

    let mut keys = [false; 256];
    let mut slots = [false; 256];
    let mut keys_by_slot = [None; 256];
    for branch in &node.branches {
        if core::mem::replace(&mut keys[branch.key as usize], true) {
            return Err(Error::DuplicateKey { key: branch.key });
        }

        let slot = branch.slot as usize;
        if slot >= node.kind.capacity() || core::mem::replace(&mut slots[slot], true) {
            return Err(Error::InvalidSlot {
                kind: node.kind,
                slot: branch.slot,
            });
        }

        match node.kind {
            NodeKind::N4 | NodeKind::N16 => {
                if slot >= node.branches.len() {
                    return Err(Error::InvalidSlot {
                        kind: node.kind,
                        slot: branch.slot,
                    });
                }
                keys_by_slot[slot] = Some(branch.key);
            }
            NodeKind::N48 => {}
            NodeKind::N256 if slot == branch.key as usize => {}
            NodeKind::N256 => {
                return Err(Error::InvalidSlot {
                    kind: node.kind,
                    slot: branch.slot,
                });
            }
        }

        let child_path = path_bytes + 1;
        match &branch.child {
            Child::Value(_) if child_path == 8 => {}
            Child::Value(_) => {
                return Err(Error::InvalidKeyLength {
                    found: child_path,
                    expected: 8,
                });
            }
            Child::Node(child) if child_path < 8 => validate_node(child, child_path)?,
            Child::Node(_) => {
                return Err(Error::InvalidKeyLength {
                    found: child_path + 1,
                    expected: 8,
                });
            }
        }
    }

    match node.kind {
        NodeKind::N4 | NodeKind::N16 => {
            if !node.free_slots.is_empty()
                || slots[..node.branches.len()]
                    .iter()
                    .any(|occupied| !occupied)
                || keys_by_slot[..node.branches.len()]
                    .windows(2)
                    .any(|keys| keys[0] >= keys[1])
            {
                return Err(Error::InvalidFreeList);
            }
        }
        NodeKind::N48 => {
            let mut free = [false; 48];
            for slot in &node.free_slots {
                let slot = *slot as usize;
                if slot >= 48 || slots[slot] || core::mem::replace(&mut free[slot], true) {
                    return Err(Error::InvalidFreeList);
                }
            }
            if node.branches.len() + node.free_slots.len() != 48 {
                return Err(Error::InvalidFreeList);
            }
        }
        NodeKind::N256 if !node.free_slots.is_empty() => {
            return Err(Error::InvalidFreeList);
        }
        NodeKind::N256 => {}
    }

    Ok(())
}

fn export_node<T>(
    pointer: NonNull<BaseNode>,
    encode: &mut impl FnMut(usize) -> T,
) -> Result<Node<T>, Error> {
    let guard = BaseNode::read_lock(pointer).map_err(|_| Error::ConcurrentMutation)?;
    let kind = NodeKind::from_raw(guard.as_ref().get_type());
    let prefix = guard.as_ref().prefix().to_vec();
    let free_slots = guard.as_ref().topology_free_slots();
    let children = guard.as_ref().topology_children();
    let mut branches = Vec::with_capacity(children.len());

    for (key, slot, child) in children {
        let child = cast_ptr!(child => {
            Payload(payload) => Child::Value(encode(payload)),
            SubNode(node) => Child::Node(export_node(node, encode)?),
        });
        branches.push(Branch { key, slot, child });
    }
    guard
        .check_version()
        .map_err(|_| Error::ConcurrentMutation)?;
    branches.sort_unstable_by_key(|branch| branch.slot);

    Ok(Node {
        kind,
        prefix,
        branches,
        free_slots,
    })
}

struct BuiltNode<'a, A: Allocator> {
    pointer: Option<NonNull<BaseNode>>,
    allocator: &'a A,
    drain: &'a Arc<dyn Fn([u8; 8], usize)>,
}

impl<A: Allocator> BuiltNode<'_, A> {
    fn into_inner(mut self) -> NonNull<BaseNode> {
        self.pointer.take().unwrap()
    }
}

impl<A: Allocator> Drop for BuiltNode<'_, A> {
    fn drop(&mut self) {
        if let Some(pointer) = self.pointer.take() {
            unsafe {
                destroy_tree(
                    pointer,
                    &mut Vec::new(),
                    self.allocator,
                    self.drain.as_ref(),
                )
            };
        }
    }
}

fn import_node<'a, T, A: Allocator>(
    mut node: Node<T>,
    allocator: &'a A,
    drain: &'a Arc<dyn Fn([u8; 8], usize)>,
    decode: &mut impl FnMut(T) -> usize,
) -> Result<BuiltNode<'a, A>, Error> {
    let pointer = BaseNode::make_topology_node(node.kind.into_raw(), &node.prefix, allocator)
        .map_err(|_| Error::OutOfMemory)?;
    let built = BuiltNode {
        pointer: Some(pointer),
        allocator,
        drain,
    };
    node.branches.sort_unstable_by_key(|branch| branch.slot);

    for branch in node.branches {
        let child = match branch.child {
            Child::Value(value) => NodePtr::from_payload(decode(value)),
            Child::Node(node) => {
                let child = import_node(node, allocator, drain, decode)?;
                NodePtr::from_node(child.into_inner())
            }
        };
        unsafe { built.pointer.unwrap().as_mut() }.restore_topology_branch(
            branch.key,
            branch.slot,
            child,
        );
    }
    unsafe { built.pointer.unwrap().as_mut() }.restore_topology_finish(&node.free_slots);
    Ok(built)
}

unsafe fn destroy_tree<A: Allocator>(
    pointer: NonNull<BaseNode>,
    key: &mut Vec<u8>,
    allocator: &A,
    drain: &dyn Fn([u8; 8], usize),
) {
    let node = unsafe { pointer.as_ref() };
    let prefix_len = node.prefix().len();
    key.extend_from_slice(node.prefix());

    for (byte, child) in node.get_children(0, 255) {
        key.push(byte);
        cast_ptr!(child => {
            Payload(payload) => {
                let mut full_key = [0; 8];
                full_key[..key.len()].copy_from_slice(key);
                drain(full_key, payload);
            },
            SubNode(child) => unsafe {
                destroy_tree(child, key, allocator, drain);
            },
        });
        key.pop();
    }

    key.truncate(key.len() - prefix_len);
    let layout = node.get_type().node_layout();
    let pointer = NonNull::new(pointer.as_ptr().cast::<u8>()).unwrap();
    unsafe { allocator.deallocate(pointer, layout) };
}

#[cfg(test)]
mod tests {
    use std::alloc::Layout;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::error::OOMError;

    #[derive(Clone)]
    struct FailingAllocator {
        remaining: Arc<AtomicUsize>,
        live_bytes: Arc<AtomicUsize>,
    }

    impl Allocator for FailingAllocator {
        fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, OOMError> {
            if self
                .remaining
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |remaining| {
                    remaining.checked_sub(1)
                })
                .is_err()
            {
                return Err(OOMError::new());
            }

            let pointer = unsafe { std::alloc::alloc(layout) };
            let pointer = NonNull::new(std::ptr::slice_from_raw_parts_mut(pointer, layout.size()))
                .ok_or_else(OOMError::new)?;
            self.live_bytes.fetch_add(layout.size(), Ordering::Relaxed);
            Ok(pointer)
        }

        unsafe fn deallocate(&self, pointer: NonNull<u8>, layout: Layout) {
            self.live_bytes.fetch_sub(layout.size(), Ordering::Relaxed);
            unsafe { std::alloc::dealloc(pointer.as_ptr(), layout) };
        }
    }

    fn contains_kind<V>(node: &Node<V>, expected: NodeKind) -> bool {
        node.kind == expected
            || node.branches.iter().any(|branch| match &branch.child {
                Child::Value(_) => false,
                Child::Node(node) => contains_kind(node, expected),
            })
    }

    fn assert_round_trip(count: usize, expected: NodeKind) {
        let mut tree = CongeeRaw::<usize, usize>::default();
        let guard = tree.pin();
        for key in 0..count {
            tree.insert(key, key ^ 0xA5A5, &guard).unwrap();
        }
        drop(guard);

        let before = tree.export_topology(|value| value).unwrap();
        assert!(contains_kind(&before.root, expected));
        let mut restored =
            CongeeRaw::<usize, usize>::from_topology_default(before.clone(), |value| value)
                .unwrap();
        let after = restored.export_topology(|value| value).unwrap();
        assert_eq!(after, before);

        let guard = restored.pin();
        for key in 0..count {
            assert_eq!(restored.get(&key, &guard), Some(key ^ 0xA5A5));
        }
    }

    #[test]
    fn preserves_every_adaptive_node_kind() {
        assert_round_trip(2, NodeKind::N4);
        assert_round_trip(10, NodeKind::N16);
        assert_round_trip(32, NodeKind::N48);
        assert_round_trip(96, NodeKind::N256);
    }

    #[test]
    fn preserves_node48_free_list_after_removals() {
        let mut tree = CongeeRaw::<usize, usize>::default();
        let guard = tree.pin();
        for key in 0..40 {
            tree.insert(key, key + 1, &guard).unwrap();
        }
        for key in [3, 7, 11, 19, 23] {
            tree.remove(&key, &guard).unwrap();
        }
        drop(guard);

        let before = tree.export_topology(|value| value).unwrap();
        let mut restored =
            CongeeRaw::<usize, usize>::from_topology_default(before.clone(), |value| value)
                .unwrap();
        let after = restored.export_topology(|value| value).unwrap();
        assert_eq!(after, before);
    }

    #[test]
    fn allocation_failure_cleans_partial_tree_and_values() {
        let mut source = CongeeRaw::<usize, usize>::default();
        let guard = source.pin();
        for byte in 0..96usize {
            let key = byte << 56;
            source.insert(key, byte + 1, &guard).unwrap();
        }
        drop(guard);
        let topology = source.export_topology(|value| value).unwrap();

        let live_bytes = Arc::new(AtomicUsize::new(0));
        let allocator = FailingAllocator {
            remaining: Arc::new(AtomicUsize::new(5)),
            live_bytes: Arc::clone(&live_bytes),
        };
        let decoded = Arc::new(AtomicUsize::new(0));
        let drained = Arc::new(AtomicUsize::new(0));
        let decoded_for_restore = Arc::clone(&decoded);
        let drained_for_restore = Arc::clone(&drained);

        let restored = CongeeRaw::<usize, usize, FailingAllocator>::from_topology_with_drainer(
            topology,
            allocator,
            move |value| {
                decoded_for_restore.fetch_add(1, Ordering::Relaxed);
                value
            },
            move |_key, _value| {
                drained_for_restore.fetch_add(1, Ordering::Relaxed);
            },
        );

        assert_eq!(restored.err(), Some(Error::OutOfMemory));
        assert_eq!(live_bytes.load(Ordering::Relaxed), 0);
        assert!(decoded.load(Ordering::Relaxed) > 0);
        assert_eq!(
            decoded.load(Ordering::Relaxed),
            drained.load(Ordering::Relaxed)
        );
    }
}
