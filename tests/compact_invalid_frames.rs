use congee::CongeeCompactSet;

const QUERY: usize = 0x2101_0203_0405_0607;

fn n4_internal(target: u32, child: &[u8]) -> Vec<u8> {
    let mut bytes = vec![0, 0, 0x21];
    bytes.extend_from_slice(&target.to_le_bytes());
    bytes.extend_from_slice(child);
    bytes
}

fn n4_leaf_for_query() -> Vec<u8> {
    let mut bytes = vec![0x34, 0]; // The parent consumed one byte; six precede the leaf key.
    bytes.extend_from_slice(&QUERY.to_be_bytes()[1..7]);
    bytes.push(QUERY as u8); // Final key byte.
    bytes
}

#[test]
fn selected_internal_edges_must_target_a_later_bounded_frame() {
    let child = n4_leaf_for_query();
    let valid = n4_internal(7, &child);
    assert!(CongeeCompactSet::<usize>::new(&valid).contains(&QUERY));

    for target in [0, 2, 6, 100] {
        let bytes = n4_internal(target, &child);
        assert!(!CongeeCompactSet::<usize>::new(&bytes).contains(&QUERY));
    }

    let truncated = n4_internal(7, &[0x3c, 0]);
    assert!(!CongeeCompactSet::<usize>::new(&truncated).contains(&QUERY));
}

#[test]
fn a_child_cannot_point_back_before_its_own_frame() {
    let mut child = n4_internal(2, &[]);
    child[2] = QUERY.to_be_bytes()[1];
    let bytes = n4_internal(7, &child);
    assert!(!CongeeCompactSet::<usize>::new(&bytes).contains(&QUERY));
}

#[test]
fn internal_edges_cannot_follow_the_final_key_byte() {
    let mut bytes = vec![0x38, 0]; // N4 internal with a seven-byte prefix.
    bytes.extend_from_slice(&QUERY.to_be_bytes()[..7]);
    bytes.push(QUERY.to_be_bytes()[7]);
    bytes.extend_from_slice(&0u32.to_le_bytes()); // An internal edge cannot terminate a key.
    bytes.extend_from_slice(&[0x3c, 0]);
    bytes.extend_from_slice(&QUERY.to_be_bytes()[1..]);
    bytes.push(QUERY.to_be_bytes()[7]);
    assert!(!CongeeCompactSet::<usize>::new(&bytes).contains(&QUERY));
}

#[test]
fn provided_in_body_offset_is_rejected() {
    let bytes = [0, 1, 0x21, 0x22, 8, 0, 0, 0, 0x34, 0, 1, 2, 3, 4, 5, 6, 7];
    let query = 0x2101_0203_0405_0607usize;
    assert!(!CongeeCompactSet::<usize>::new(&bytes).contains(&query));
}

fn bitmap_frame(kind: u8, prefix_len: u8) -> Vec<u8> {
    let mut bytes = vec![(kind & 7) | (prefix_len << 3), 0];
    let prefix_end = bytes.len() + prefix_len as usize;
    bytes.resize(prefix_end, 0);
    if kind == 2 {
        bytes.extend_from_slice(&[0; 4]); // N48 internal popcount cache.
    }
    let bitmap_at = bytes.len();
    bytes.extend_from_slice(&[0; 32]);
    bytes[bitmap_at] = 0b11; // Two bits, while the header declares one child.
    if kind == 2 {
        bytes.extend_from_slice(&[0; 4]);
    }
    bytes
}

#[test]
fn bitmap_cardinality_mismatch_stops_inspection_at_that_frame() {
    let preceding_complete_leaf = [4, 0, 0xaa];
    for (kind, prefix_len) in [(2, 0), (6, 7), (7, 7)] {
        let malformed = bitmap_frame(kind, prefix_len);
        assert!(!CongeeCompactSet::<usize>::new(&malformed).contains(&0));

        let mut bytes = preceding_complete_leaf.to_vec();
        bytes.extend_from_slice(&malformed);
        let compact = CongeeCompactSet::<usize>::new(&bytes);
        assert_eq!(compact.node_count(), 1, "node type {kind}");
        let stats = compact.stats();
        assert_eq!(stats.total_nodes, 1, "node type {kind}");
        assert_eq!(stats.n4_leaf_count, 1, "node type {kind}");
        assert!(!compact.contains(&0));
    }
}

#[test]
fn truncated_n4_frame_is_rejected_by_lookup_and_inspection() {
    let bytes = [0, 0, 0x21, 7, 0, 0];
    let compact = CongeeCompactSet::<usize>::new(&bytes);
    assert!(!compact.contains(&QUERY));
    assert_eq!(compact.node_count(), 0);
    assert_eq!(compact.stats().total_nodes, 0);
}
