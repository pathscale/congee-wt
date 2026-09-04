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
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const RECLAIM_BATCH: usize = 256;

    struct Reclaimer {
        domain: ps_reclaim::Domain,
        pending: AtomicUsize,
    }

    impl Reclaimer {
        fn global() -> &'static Self {
            static RECLAIMER: OnceLock<Reclaimer> = OnceLock::new();
            RECLAIMER.get_or_init(|| Self {
                domain: ps_reclaim::Domain::new(),
                pending: AtomicUsize::new(0),
            })
        }

        fn advance(&self) -> usize {
            let reclaimed = self.domain.advance();
            if reclaimed != 0 {
                self.pending.fetch_sub(reclaimed, Ordering::AcqRel);
            }
            reclaimed
        }

        fn advance_if_needed(&self) {
            if self.pending.load(Ordering::Acquire) >= RECLAIM_BATCH {
                self.advance();
            }
        }
    }

    /// A pin in Congee's reclamation domain.
    pub struct Guard {
        reclaimer: &'static Reclaimer,
        _pin: ps_reclaim::Guard<'static>,
    }

    impl Guard {
        /// Defers destruction until every reader that predates this call has
        /// left the Congee reclamation domain.
        pub fn defer<F>(&self, reclaim: F)
        where
            F: FnOnce() + Send + 'static,
        {
            self.reclaimer.domain.retire(reclaim);
            self.reclaimer.pending.fetch_add(1, Ordering::Release);
        }

        /// Attempts one non-blocking reclamation pass.
        pub fn flush(&self) -> usize {
            self.reclaimer.advance()
        }
    }

    /// Pins the calling thread in Congee's reclamation domain.
    #[inline]
    pub fn pin() -> Guard {
        let reclaimer = Reclaimer::global();
        reclaimer.advance_if_needed();
        Guard {
            reclaimer,
            _pin: reclaimer.domain.pin(),
        }
    }

    /// Attempts one non-blocking reclamation pass.
    pub fn collect() -> usize {
        Reclaimer::global().advance()
    }

    /// Returns the current number of pending retirements.
    pub fn pending_retirements() -> usize {
        Reclaimer::global().pending.load(Ordering::Acquire)
    }
}

pub use congee::Congee;
pub use congee_compact_set::{CompactSetStats, CongeeCompactSet};
pub use congee_raw::CongeeRaw;
pub use congee_set::CongeeSet;
pub use utils::{Allocator, DefaultAllocator, MemoryStatsAllocator};
