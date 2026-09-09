# crossbeam-epoch to ps-reclaim

Status: **already done and published.** The port landed in `congee-wt` 0.4.4 and
was refined in 0.4.5. `0.4.3` is the last release that links `crossbeam-epoch`.
This note is the record of what the change was, because the task that produced
it predates the work.

Verified against the working tree at `/Users/revenge/code/congee-wt` (on
`master`) and against the published sources in the local registry cache:

    ~/.cargo/registry/src/index.crates.io-*/congee-wt-0.4.3/   crossbeam-epoch
    ~/.cargo/registry/src/index.crates.io-*/congee-wt-0.4.4/   ps-reclaim
    ~/.cargo/registry/src/index.crates.io-*/congee-wt-0.4.5/   ps-reclaim, no_std

There is no `crossbeam-epoch` left anywhere in the crate. The only remaining
occurrence of the string `crossbeam` in the source is a comment in `src/lock.rs`
crediting the seqlock read pattern, and one in `src/utils.rs` crediting the
backoff. `crossbeam-utils` still appears in `Cargo.lock` as a transitive
dev-dependency of the benchmark harness, not of the library.

## What changed

`Cargo.toml`

    -crossbeam-epoch = "0.9.18"
    +ps-reclaim = { version = "0.1.4", default-features = false, features = ["libc", "spin"] }

with `std = ["ps-reclaim/std", "serde?/std"]` in `[features]`. Dropping
`crossbeam-epoch` is what made the `no_std` build in 0.4.5 possible.

`src/lib.rs`: `pub mod epoch` stopped being a re-export and became a real
module.

    // 0.4.3
    pub mod epoch {
        pub use crossbeam_epoch::{Guard, pin};
    }

It now holds three things:

- `Reclaimer`, crate-private, one `ps_reclaim::Domain` plus an `AtomicUsize`
  count of outstanding retirements.
- `epoch::Guard<'a>`, public, wrapping `ps_reclaim::Guard<'a>` and a borrow of
  the `Reclaimer` it came from. It exposes `defer` and `flush`.
- `epoch::pin_in(&Reclaimer)`, crate-private, which runs one bounded
  reclamation pass before pinning.

`src/congee_inner.rs`: `CongeeInner` gained a `reclaimer: Reclaimer` field, so
each tree owns its grace period rather than sharing one process-global epoch.
`pin()` returns a guard into that tree's domain; `assert_guard` panics if a
guard from another tree is passed in. `Drop` drains the tree's domain with
`while self.reclaimer.advance() != 0 {}` instead of the old
`crossbeam_epoch::pin().flush()`.

Call sites in `src/congee.rs`, `src/congee_raw.rs`, `src/congee_set.rs`,
`src/congee_compact_set.rs`, `src/stats.rs` and `bench/basic.rs` changed only in
that `Guard` now names the local type and carries a lifetime.

## The breaking API change

Two breaks, both in `congee::epoch`:

1. `epoch::Guard` gained a lifetime parameter and is a different type. It was
   `crossbeam_epoch::Guard` (no lifetime, `Send`); it is now
   `congee::epoch::Guard<'a>`, borrowing the tree, and `!Send` and `!Sync`
   because `ps_reclaim::Guard` is `!Send` by construction (its participant
   pointer is a raw pointer, so the auto traits do not apply). A guard can no
   longer be moved to another thread, and its owning tree can no longer be
   dropped while it is live.

2. The free function `epoch::pin()` is gone. There is no unattached pin any
   more: a guard has to come from a specific tree's `pin()` method, and using
   one tree's guard against another now panics rather than silently working.

What did not change is the path. `congee::epoch::Guard` still resolves, and in
reference position the new lifetime is elided, so `&congee::epoch::Guard` is
still valid syntax. `Guard::defer` kept a compatible bound
(`FnOnce() + Send + 'static`; crossbeam's was `FnOnce() -> R + Send + 'static`)
and `Guard::flush` kept its name. `defer_unchecked`, `repin` and `unprotected`
have no replacement, and nothing in this crate used them.

## The WorkTable call site

`/Users/revenge/code/WorkTable/src/index/congee.rs`

**No edit is required.** The TODO entry names line 101; the signature has since
moved to line 120 and reads

    fn retire_old(pointer: usize, guard: &congee::epoch::Guard) -> Arc<V> {

That compiles unchanged against 0.4.4 and 0.4.5. The elided lifetime is
inferred, and the `guard.defer(move || drop(delayed))` on the following lines
satisfies the new bound. Every other guard in that file is a local from
`self.inner.pin()` passed by reference into a call on the same tree, which is
exactly what the new tree-scoped guard requires.

WorkTable is already on the ported release: its `Cargo.lock` pins
`congee-wt 0.4.4` with `ps-reclaim` as its only dependency, and
`WorkTable/src/util/epoch.rs` re-exports `ps_reclaim::{Domain, Guard}` for
WorkTable's own domains. The two crates now share one reclamation
implementation without sharing a domain.

The only thing left on the WorkTable side is a version floor, if 0.4.5 is
wanted for the `no_std` build. The requirement in `WorkTable/Cargo.toml:41` is
currently

    congee = { package = "congee-wt", version = "^0.4, >=0.4.4" }

which already excludes every crossbeam release.
