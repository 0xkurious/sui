// Copyright (c) Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

//! Backfill batch sizing for resumable subscriptions.
//!
//! [`scan_checkpoints`] walks past checkpoints in parallel batches via LedgerService until the
//! cursor is within `transition_threshold` of the network tip. The remaining gap is left for
//! the caller to bridge by consuming the live broadcast.
//!
//! ```text
//!   start_after        last_scanned        tip - threshold      network_tip
//!        │                  │                     │                  │
//!        ▼                  ▼                     ▼                  ▼
//!   ─────[batch][batch][batch][batch][batch]──────[──── gap ──────────]
//!         (yielded, each up to max_batch_size)     (caller bridges
//!                                                   via live broadcast)
//! ```
//!
//! Each iteration:
//!   1. Read `network_tip`.
//!   2. If `tip - last_scanned > transition_threshold`, fetch the next batch (sized by
//!      [`next_backfill_batch`]) and yield each checkpoint.
//!   3. Otherwise return; the caller takes over from `last_scanned + 1`.

use std::ops::RangeInclusive;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

use async_stream::stream;
use futures::Stream;

use super::ProcessedCheckpoint;
use super::checkpoint_stream_task::checkpoint_field_mask;
use super::gap_recovery::CheckpointFetcher;
use super::gap_recovery::fetch_and_process;
use crate::error::RpcError;

/// Yield `ProcessedCheckpoint`s from `start_after + 1` onward by reading LedgerService in
/// parallel batches. Exits once the cursor is within `transition_threshold` of the network
/// tip; the caller then takes over by consuming the live broadcast (the bridge between scan
/// end and the first live item is covered by the caller subscribing before this scan starts).
pub(crate) fn scan_checkpoints<F: CheckpointFetcher>(
    fetcher: Arc<F>,
    network_tip: Arc<AtomicU64>,
    start_after: u64,
    max_batch_size: usize,
    transition_threshold: u64,
) -> impl Stream<Item = Result<Arc<ProcessedCheckpoint>, RpcError>> {
    stream! {
        let mask = checkpoint_field_mask();
        let mut last_scanned = start_after;
        loop {
            let tip = network_tip.load(Ordering::Relaxed);
            let Some(batch) =
                next_backfill_batch(last_scanned, tip, max_batch_size, transition_threshold)
            else {
                break;
            };
            match fetch_and_process(fetcher.as_ref(), &mask, batch).await {
                Ok(processed) => {
                    for cp in processed {
                        last_scanned = cp.sequence_number;
                        yield Ok(cp);
                    }
                }
                Err(e) => {
                    yield Err(e.into());
                    return;
                }
            }
        }
    }
}

/// Pick the next batch of checkpoints to scan, or signal that we are close enough to the tip
/// to hand off to the live broadcast.
///
/// During the backfill phase, the caller repeatedly asks this function "what should I scan
/// next?" and feeds the returned range to a parallel LedgerService read. The batch grows up
/// to `max_batch_size` but never exceeds `tip_checkpoint`, so the trailing batch naturally
/// shrinks as we approach the tip.
///
/// Once the remaining gap (`tip_checkpoint - last_scanned_checkpoint`) is at or below
/// `transition_threshold`, the function returns `None`. That signal tells the caller to stop
/// scanning and switch to consuming the live broadcast.
fn next_backfill_batch(
    last_scanned_checkpoint: u64,
    tip_checkpoint: u64,
    max_batch_size: usize,
    transition_threshold: u64,
) -> Option<RangeInclusive<u64>> {
    let gap = tip_checkpoint.saturating_sub(last_scanned_checkpoint);
    if gap <= transition_threshold {
        return None;
    }
    let lo = last_scanned_checkpoint + 1;
    let hi = lo
        .saturating_add(max_batch_size as u64)
        .saturating_sub(1)
        .min(tip_checkpoint);
    Some(lo..=hi)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn within_threshold_returns_none() {
        // gap = 5, threshold = 10 → already close enough.
        assert!(
            next_backfill_batch(
                /* last_scanned_checkpoint = */ 100, /* tip_checkpoint = */ 105,
                /* max_batch_size = */ 50, /* transition_threshold = */ 10,
            )
            .is_none()
        );
    }

    #[test]
    fn full_batch_when_far_behind() {
        assert_eq!(
            next_backfill_batch(
                /* last_scanned_checkpoint = */ 100, /* tip_checkpoint = */ 1000,
                /* max_batch_size = */ 50, /* transition_threshold = */ 10,
            ),
            Some(101..=150),
        );
    }

    #[test]
    fn trailing_batch_clamps_to_tip() {
        // Gap (30) > threshold (10), but only 30 checkpoints left to fetch.
        assert_eq!(
            next_backfill_batch(
                /* last_scanned_checkpoint = */ 100, /* tip_checkpoint = */ 130,
                /* max_batch_size = */ 50, /* transition_threshold = */ 10,
            ),
            Some(101..=130),
        );
    }

    #[test]
    fn threshold_boundary_inclusive() {
        // gap = 10, threshold = 10 → at threshold, no more backfill.
        assert!(
            next_backfill_batch(
                /* last_scanned_checkpoint = */ 100, /* tip_checkpoint = */ 110,
                /* max_batch_size = */ 50, /* transition_threshold = */ 10,
            )
            .is_none()
        );
    }
}
