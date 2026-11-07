use congee::{CongeeCompactSet, CongeeSet};

#[test]
fn snapshot_prunes_removed_subtrees() {
    for keep_sibling in [false, true] {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let set = CongeeSet::<usize>::default();
            let guard = set.pin();
            let first = 0x0101_0000_0000_0000usize;
            let second = 0x0102_0000_0000_0000usize;
            let sibling = 0x0201_0000_0000_0000usize;
            set.insert(first, &guard).unwrap();
            set.insert(second, &guard).unwrap();
            if keep_sibling {
                set.insert(sibling, &guard).unwrap();
            }
            assert!(set.remove(&first, &guard));
            assert!(set.remove(&second, &guard));
            let bytes = set.to_compact_set();
            let compact = CongeeCompactSet::<usize>::new(&bytes);
            assert!(!compact.contains(&first));
            assert!(!compact.contains(&second));
            assert_eq!(compact.contains(&sibling), keep_sibling);
            if !keep_sibling {
                assert!(bytes.is_empty());
            }
            tx.send(()).unwrap();
        });
        rx.recv_timeout(std::time::Duration::from_secs(3)).unwrap();
    }
}
