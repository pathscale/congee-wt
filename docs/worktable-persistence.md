# Pointer-free Congee topology for WorkTable

Status: implementation in progress on `feat/worktable-persistence`.

## Contract

`congee-wt` exposes a typed topology interchange representation; WorkTable owns
the durable page framing, checksums, generation protocol, and logical WAL.

The representation records Congee's own physical structure: path-compressed
node prefixes, N4/N16/N48/N256 kinds, live child slots, and the N48 free-list in
next-allocation order. Values are encoded by the caller. It never records raw
node pointers, locks, versions, epochs, allocator state, or pointer-sized value
payloads.

Export requires an exclusive mutable borrow. Import validates the entire shape
before allocation and owns partial subtrees so allocator failure cleans up nodes
and decoded values. Neither path adds a field or branch to Congee point
operations.

WorkTable should retain a monotonically sequenced logical WAL before beginning a
quiescent checkpoint. Recovery restores this backend-native topology and then
replays later `Set` and `Remove` mutations.
