#![doc = include_str!("../README.md")]
#![allow(clippy::comparison_chain)]
#![allow(clippy::len_without_is_empty)]
#![cfg_attr(docsrs, feature(doc_cfg))]

mod congee;
pub mod congee_compact_set;
mod congee_inner;
mod congee_raw;
mod congee_set;
mod error;
mod lock;
mod nodes;
mod range_scan;
mod stats;
pub mod topology;
mod utils;
use congee_inner::CongeeInner;

#[cfg(test)]
mod tests;

/// Types needed to safely access shared data concurrently.
pub mod epoch {
    use std::sync::atomic::{AtomicUsize, Ordering};

    const RECLAIM_BATCH: usize = 256;

    pub(crate) struct Reclaimer {
        domain: ps_reclaim::Domain,
        pending: AtomicUsize,
    }

    impl Reclaimer {
        pub(crate) fn new() -> Self {
            Self {
                domain: ps_reclaim::Domain::new(),
                pending: AtomicUsize::new(0),
            }
        }

        pub(crate) fn advance(&self) -> usize {
            let reclaimed = self.domain.advance();
            if reclaimed != 0 {
                self.pending.fetch_sub(reclaimed, Ordering::Relaxed);
            }
            reclaimed
        }

        fn advance_up_to(&self, limit: usize) -> usize {
            let reclaimed = self.domain.advance_up_to(limit);
            if reclaimed != 0 {
                self.pending.fetch_sub(reclaimed, Ordering::Relaxed);
            }
            reclaimed
        }

        fn advance_if_needed(&self) {
            if self.pending.load(Ordering::Relaxed) >= RECLAIM_BATCH {
                self.advance_up_to(RECLAIM_BATCH);
            }
        }
    }

    /// A pin in Congee's reclamation domain.
    pub struct Guard<'a> {
        reclaimer: &'a Reclaimer,
        _pin: ps_reclaim::Guard<'a>,
    }

    impl Guard<'_> {
        /// Defers destruction until every reader that predates this call has
        /// left the Congee reclamation domain.
        pub fn defer<F>(&self, reclaim: F)
        where
            F: FnOnce() + Send + 'static,
        {
            // Account first: once published to the domain, another thread may
            // immediately run the callback and decrement the pending count.
            self.reclaimer.pending.fetch_add(1, Ordering::Relaxed);
            self.reclaimer.domain.retire(reclaim);
        }

        /// Attempts one non-blocking reclamation pass.
        pub fn flush(&self) -> usize {
            self.reclaimer.advance()
        }

        pub(crate) fn belongs_to(&self, reclaimer: &Reclaimer) -> bool {
            std::ptr::eq(self.reclaimer, reclaimer)
        }
    }

    pub(crate) fn pin_in(reclaimer: &Reclaimer) -> Guard<'_> {
        reclaimer.advance_if_needed();
        Guard {
            reclaimer,
            _pin: reclaimer.domain.pin(),
        }
    }

    #[cfg(test)]
    mod tests {
        use super::{RECLAIM_BATCH, Reclaimer, pin_in};
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        #[test]
        fn automatic_collection_is_bounded_to_one_batch() {
            let reclaimer = Reclaimer::new();
            let hits = Arc::new(AtomicUsize::new(0));
            for _ in 0..RECLAIM_BATCH * 3 {
                reclaimer.pending.fetch_add(1, Ordering::Relaxed);
                let hits = hits.clone();
                reclaimer.domain.retire(move || {
                    hits.fetch_add(1, Ordering::Relaxed);
                });
            }

            reclaimer.advance_if_needed();

            assert_eq!(hits.load(Ordering::Relaxed), RECLAIM_BATCH);
            assert_eq!(reclaimer.pending.load(Ordering::Relaxed), RECLAIM_BATCH * 2);
        }

        #[test]
        fn reader_of_one_tree_does_not_delay_another_tree() {
            let first = Reclaimer::new();
            let second = Reclaimer::new();
            let _first_reader = pin_in(&first);
            let hits = Arc::new(AtomicUsize::new(0));

            {
                let second_writer = pin_in(&second);
                let hits = hits.clone();
                second_writer.defer(move || {
                    hits.fetch_add(1, Ordering::Relaxed);
                });
            }

            second.advance();
            assert_eq!(hits.load(Ordering::Relaxed), 1);
        }
    }
}

pub use congee::Congee;
pub use congee_compact_set::{CompactSetStats, CongeeCompactSet};
pub use congee_raw::CongeeRaw;
pub use congee_set::CongeeSet;
pub use utils::{Allocator, DefaultAllocator, MemoryStatsAllocator};
