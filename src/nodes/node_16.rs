use super::{
    NodePtr,
    base_node::{BaseNode, Node, NodeIter, NodeType},
};
use alloc::vec::Vec;

#[cfg(all(target_arch = "x86_64", target_feature = "sse2"))]
use core::arch::x86_64::*;

#[repr(C)]
#[repr(align(8))] // Node 16 doesn't need to align to 64 bc it occupies 3 cache lines anyway
pub(crate) struct Node16 {
    base: BaseNode,
    children: [NodePtr; 16],
    keys: [u8; 16],
}

#[cfg(not(feature = "shuttle"))]
const _: () = assert!(core::mem::size_of::<Node16>() == 160);

#[cfg(not(feature = "shuttle"))]
const _: () = assert!(core::mem::align_of::<Node16>() == 8);

impl Node16 {
    fn get_insert_pos(&self, key: u8) -> usize {
        let mut pos = 0;
        while pos < self.base.meta.count() {
            if self.keys[pos] >= key {
                return pos;
            }
            pos += 1;
        }
        pos
    }

    #[cfg(all(target_arch = "x86_64", target_feature = "sse2"))]
    fn get_child_pos_simd(&self, key: u8) -> Option<usize> {
        // SAFETY: SSE2 is enabled for this target, and the unaligned load reads
        // exactly the initialized 16-byte key array. Only live slots are used.
        unsafe {
            let key_vec = _mm_set1_epi8(key as i8);
            let keys_vec = _mm_loadu_si128(self.keys.as_ptr() as *const __m128i);
            let mask = _mm_movemask_epi8(_mm_cmpeq_epi8(key_vec, keys_vec)) as u16;
            if mask != 0 {
                let pos = mask.trailing_zeros() as usize;
                if pos < self.base.meta.count() {
                    return Some(pos);
                }
            }
            None
        }
    }

    #[cfg(not(all(target_arch = "x86_64", target_feature = "sse2")))]
    fn get_child_pos_simd(&self, key: u8) -> Option<usize> {
        self.get_child_pos_fallback(key)
    }

    #[cfg(any(test, not(all(target_arch = "x86_64", target_feature = "sse2"))))]
    #[inline]
    fn get_child_pos_fallback(&self, key: u8) -> Option<usize> {
        self.keys
            .iter()
            .take(self.base.meta.count())
            .position(|k| *k == key)
    }

    #[inline]
    fn get_child_pos(&self, key: u8) -> Option<usize> {
        self.get_child_pos_simd(key)
    }
}

pub(crate) struct Node16Iter<'a> {
    node: &'a Node16,
    start_pos: usize,
    end_pos: usize,
}

impl Iterator for Node16Iter<'_> {
    type Item = (u8, NodePtr);

    fn next(&mut self) -> Option<Self::Item> {
        if self.start_pos > self.end_pos {
            return None;
        }
        let key = self.node.keys[self.start_pos];
        let child = self.node.children[self.start_pos];
        self.start_pos += 1;
        Some((key, child))
    }
}

impl Node for Node16 {
    fn get_type() -> NodeType {
        NodeType::N16
    }

    fn topology_children(&self) -> Vec<(u8, u16, NodePtr)> {
        (0..self.base.meta.count())
            .map(|slot| (self.keys[slot], slot as u16, self.children[slot]))
            .collect()
    }

    fn restore_topology_branch(&mut self, key: u8, slot: u16, child: NodePtr) {
        assert_eq!(slot as usize, self.base.meta.count());
        self.insert(key, child);
    }

    fn get_children(&self, start: u8, end: u8) -> NodeIter<'_> {
        if self.base.meta.count() == 0 {
            // FIXME: the node may be empty due to deletion, this is not intended, we should fix the delete logic
            return NodeIter::N16(Node16Iter {
                node: self,
                start_pos: 1,
                end_pos: 0,
            });
        }

        // Find first position where key >= start
        let mut start_pos = self.base.meta.count();
        for i in 0..self.base.meta.count() {
            if self.keys[i] >= start {
                start_pos = i;
                break;
            }
        }

        // Find last position where key <= end
        let mut end_pos = 0;
        let mut found_end = false;
        for i in 0..self.base.meta.count() {
            if self.keys[i] <= end {
                end_pos = i;
                found_end = true;
            } else {
                break;
            }
        }

        // If no valid range found, return empty iterator
        if start_pos >= self.base.meta.count() || !found_end || start_pos > end_pos {
            return NodeIter::N16(Node16Iter {
                node: self,
                start_pos: 1,
                end_pos: 0,
            });
        }

        NodeIter::N16(Node16Iter {
            node: self,
            start_pos,
            end_pos,
        })
    }

    fn remove(&mut self, k: u8) {
        let pos = self
            .get_child_pos(k)
            .expect("trying to delete a non-existing key");

        self.keys.copy_within(pos + 1..self.base.meta.count(), pos);
        self.children
            .copy_within(pos + 1..self.base.meta.count(), pos);

        self.base.meta.dec_count();
        debug_assert!(self.get_child(k).is_none());
    }

    fn copy_to<N: Node>(&self, dst: &mut N) {
        for i in 0..self.base.meta.count() {
            dst.insert(self.keys[i], self.children[i]);
        }
    }

    fn base(&self) -> &BaseNode {
        &self.base
    }

    fn is_full(&self) -> bool {
        self.base.meta.count() == 16
    }

    // Insert must keep keys sorted, is this necessary?
    fn insert(&mut self, key: u8, node: NodePtr) {
        let pos = self.get_insert_pos(key);

        if pos < self.base.meta.count() {
            self.keys.copy_within(pos..self.base.meta.count(), pos + 1);
            self.children
                .copy_within(pos..self.base.meta.count(), pos + 1);
        }

        self.keys[pos] = key;
        self.children[pos] = node;
        self.base.meta.inc_count();

        assert!(self.base.meta.count() <= 16);
    }

    fn change(&mut self, key: u8, val: NodePtr) -> NodePtr {
        let pos = self.get_child_pos(key).unwrap();
        let old = self.children[pos];
        self.children[pos] = val;
        old
    }

    fn get_child(&self, key: u8) -> Option<NodePtr> {
        let pos = self.get_child_pos(key)?;
        let child = unsafe { self.children.get_unchecked(pos) };
        Some(*child)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_node() -> Node16 {
        Node16 {
            base: BaseNode::new(NodeType::N16, &[]),
            children: [NodePtr::from_payload(0); 16],
            keys: [0; 16],
        }
    }

    #[test]
    fn lookup_matches_scalar_for_all_keys_and_live_counts() {
        let mut node = create_test_node();
        for count in 0..=16 {
            for key in 0..=255 {
                assert_eq!(
                    node.get_child_pos_simd(key),
                    node.get_child_pos_fallback(key)
                );
            }
            if count < 16 {
                node.insert((count * 17) as u8, NodePtr::from_payload(count + 1));
            }
        }
    }

    #[test]
    fn test_node_operations() {
        let mut node = create_test_node();
        let ptr1 = NodePtr::from_payload(0x1000);
        let ptr2 = NodePtr::from_payload(0x2000);
        let ptr3 = NodePtr::from_payload(0x3000);

        assert_eq!(Node16::get_type(), NodeType::N16);
        assert!(!node.is_full());
        assert_eq!(node.base().meta.count(), 0);

        node.insert(20, ptr2);
        node.insert(10, ptr1);
        node.insert(30, ptr3);

        assert_eq!(node.base().meta.count(), 3);
        assert_eq!(node.keys[0], 10); // Should be sorted
        assert_eq!(node.keys[1], 20);
        assert_eq!(node.keys[2], 30);

        assert!(node.get_child(10).is_some());
        assert!(node.get_child(20).is_some());
        assert!(node.get_child(30).is_some());
        assert!(node.get_child(15).is_none());

        let new_ptr = NodePtr::from_payload(0x5000);
        let _old_ptr = node.change(10, new_ptr);
        assert!(node.get_child(10).is_some());
        assert_eq!(node.base().meta.count(), 3); // Count unchanged

        node.remove(20);
        assert_eq!(node.base().meta.count(), 2);
        assert!(node.get_child(20).is_none());
        assert_eq!(node.keys[1], 30); // Elements shifted
    }

    #[test]
    fn test_capacity_and_fullness() {
        let mut node = create_test_node();

        for i in 0..16 {
            node.insert((i * 2) as u8, NodePtr::from_payload((i + 1) * 0x1000));
            assert_eq!(node.base().meta.count(), (i + 1));
        }

        assert!(node.is_full());
        assert_eq!(node.base().meta.count(), 16);

        for i in 0..16 {
            assert!(node.get_child((i * 2) as u8).is_some());
        }
    }

    #[test]
    fn test_iterators_and_copy() {
        let mut src_node = create_test_node();
        let mut dst_node = create_test_node();
        let ptr1 = NodePtr::from_payload(0x1000);
        let ptr2 = NodePtr::from_payload(0x2000);
        let ptr3 = NodePtr::from_payload(0x3000);

        src_node.insert(10, ptr1);
        src_node.insert(30, ptr2);
        src_node.insert(50, ptr3);

        let iter = src_node.get_children(0, 255);
        if let NodeIter::N16(mut n16_iter) = iter {
            let first = n16_iter.next();
            assert!(first.is_some());
            let second = n16_iter.next();
            assert!(second.is_some());
            let third = n16_iter.next();
            assert!(third.is_some());
            assert!(n16_iter.next().is_none());
        } else {
            panic!("Expected N16 iterator");
        }

        let iter = src_node.get_children(25, 45);
        if let NodeIter::N16(mut n16_iter) = iter {
            let first = n16_iter.next();
            assert!(matches!(first, Some((30, _))));
            assert!(n16_iter.next().is_none());
        }

        src_node.copy_to(&mut dst_node);
        assert_eq!(dst_node.base().meta.count(), 3);
        assert!(dst_node.get_child(10).is_some());
        assert!(dst_node.get_child(30).is_some());
        assert!(dst_node.get_child(50).is_some());
    }

    #[test]
    fn test_edge_cases_and_search_paths() {
        let mut node = create_test_node();
        let ptr1 = NodePtr::from_payload(0x1000);

        node.insert(10, ptr1);
        let original_count = node.base().meta.count();
        assert_eq!(node.base().meta.count(), original_count);

        let empty_node = create_test_node();
        let iter = empty_node.get_children(0, 255);
        if let NodeIter::N16(mut n16_iter) = iter {
            assert!(n16_iter.next().is_none());
        }

        let mut search_node = create_test_node();
        let keys = [5, 15, 25, 35, 45, 55, 65, 75];
        for (i, &key) in keys.iter().enumerate() {
            search_node.insert(key, NodePtr::from_payload((i + 1) * 0x1000));
        }

        for &key in &keys {
            assert!(search_node.get_child(key).is_some());
        }

        assert!(search_node.get_child(1).is_none());
        assert!(search_node.get_child(100).is_none());
        assert!(search_node.get_child(40).is_none());
    }
}
