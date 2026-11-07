use congee::{CongeeCompactSet, CongeeSet};

fn key(a: u8, b: u8) -> usize {
    usize::from_be_bytes([a, b, 0, 0, 0, 0, 0, 0])
}

fn root_edge_offset(bytes: &[u8], key: u8) -> Option<u32> {
    let slope = u32::from_le_bytes(bytes[2..6].try_into().ok()?);
    let intercept = u32::from_le_bytes(bytes[6..10].try_into().ok()?);
    let residual = i16::from_le_bytes(
        bytes[10 + usize::from(key) * 2..12 + usize::from(key) * 2]
            .try_into()
            .ok()?,
    );
    if residual == i16::MIN {
        return None;
    }
    let offset = i64::from(slope) * i64::from(key) + i64::from(intercept) + i64::from(residual);
    u32::try_from(offset).ok()
}

#[test]
fn n256_sentinel_collision_preserves_every_present_edge() {
    let set = CongeeSet::<usize>::default();
    let guard = set.pin();
    for a in 0..=127 {
        for b in 0..=48 {
            set.insert(key(a, b), &guard).unwrap();
        }
    }
    for a in 128..=255 {
        set.insert(key(a, 0), &guard).unwrap();
    }

    let bytes = set
        .try_to_compact_set()
        .expect("representable compact snapshot");
    let compact = CongeeCompactSet::<usize>::new(&bytes);
    assert_eq!(u32::from_le_bytes(bytes[2..6].try_into().unwrap()), 266);
    assert_eq!(u32::from_le_bytes(bytes[6..10].try_into().unwrap()), 523);
    assert_eq!(root_edge_offset(&bytes, 0), Some(522));
    assert_eq!(root_edge_offset(&bytes, 128), Some(67_338));
    assert_eq!(root_edge_offset(&bytes, 255), Some(68_481));

    for a in 0..=127 {
        for b in 0..=48 {
            assert!(compact.contains(&key(a, b)), "missing key ({a}, {b})");
        }
        assert!(!compact.contains(&key(a, 49)));
    }
    for a in 128..=255 {
        assert!(compact.contains(&key(a, 0)));
        assert!(!compact.contains(&key(a, 1)));
    }
    assert_eq!(compact.stats().kv_pairs, 6_400);
}

#[test]
fn sparse_root_edges_use_the_reduced_slope_when_the_secant_does_not_fit() {
    let set = CongeeSet::<usize>::default();
    let guard = set.pin();
    for a in 127..=255 {
        for b in 0..=48 {
            let value = usize::from_be_bytes([a, 0, 0, 0, 0, 0, b, 0]);
            set.insert(value, &guard).unwrap();
        }
    }

    let bytes = set
        .try_to_compact_set()
        .expect("representable compact snapshot");
    let compact = CongeeCompactSet::<usize>::new(&bytes);
    assert_eq!(u32::from_le_bytes(bytes[2..6].try_into().unwrap()), 231);
    assert_eq!(root_edge_offset(&bytes, 127), Some(522));
    assert_eq!(root_edge_offset(&bytes, 255), Some(67_978));

    for a in 127..=255 {
        for b in 0..=48 {
            let value = usize::from_be_bytes([a, 0, 0, 0, 0, 0, b, 0]);
            assert!(compact.contains(&value), "missing key ({a}, {b})");
        }
        let absent = usize::from_be_bytes([a, 0, 0, 0, 0, 0, 49, 0]);
        assert!(!compact.contains(&absent));
    }
    assert_eq!(compact.stats().kv_pairs, 129 * 49);
}
