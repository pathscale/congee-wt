# Publishing congee-wt

Publishing is unavailable. The workflow publish job is disabled, and the preflight helper fails before reading package metadata or making a registry request. No release or upload path is enabled by this proposal.

The independent `registry-status VERSION` helper uses curl with default configuration disabled, a fixed HTTPS crates.io endpoint, bounded timeouts, no retries, and no redirect following. It reports only direct HTTP 200 as `already-published` and direct HTTP 404 as `absent`; transport errors and every other status fail. This status-only command does not authorize or trigger an upload.

Publishing remains disabled.
