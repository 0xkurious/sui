// Copyright (c) Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::Arc,
};

use consensus_config::{AuthorityIndex, Stake};
use consensus_types::block::{BlockDigest, BlockRef};
use parking_lot::RwLock;

use crate::{
    BlockAPI, VerifiedBlock, block::Slot, commit::LeaderStatus, context::Context,
    dag_state::DagState,
};

#[cfg(test)]
#[path = "tests/leader_slot_decider_tests.rs"]
mod leader_slot_decider_tests;

/// Stateless decision logic for v3 leader slots. Owns the read-side
/// dependencies (context + DagState handle) and exposes pure-decision
/// functions that return LeaderStatus values without touching pending
/// commit state. FlexCommitter applies the resulting state mutation.
pub(crate) struct LeaderSlotDecider {
    context: Arc<Context>,
    dag_state: Arc<RwLock<DagState>>,
}

impl LeaderSlotDecider {
    pub(crate) fn new(context: Arc<Context>, dag_state: Arc<RwLock<DagState>>) -> Self {
        Self { context, dag_state }
    }

    /// Direct-commit evaluation for `slot`. Reads next-round votes from
    /// DagState and returns Commit / Skip / Undecided.
    pub(crate) fn try_direct_decide(&self, slot: Slot) -> LeaderStatus {
        let dag_state = self.dag_state.read();
        let Some(next_round_info) = dag_state.get_round_info(slot.round + 1) else {
            // No blocks accepted in the next round yet, so there is no votes to the slot.
            return LeaderStatus::Undecided(slot);
        };
        let quorum = self.context.committee.quorum_threshold();
        if next_round_info.total_stake < quorum {
            // Next round has insufficient stake to decide the slot.
            return LeaderStatus::Undecided(slot);
        }

        let block_infos = dag_state.get_block_info_at_slot(slot);
        let num_blocks = block_infos.len();
        let mut committed = vec![];
        let mut num_skipped = 0usize;
        for (_block_ref, block_info) in block_infos {
            // Commit the leader block if it has a quorum of commit votes.
            if block_info.total_children_stake >= quorum {
                committed.push(block_info.block.clone());
            }
            // Skip the leader block if it has a quorum of skip votes.
            if next_round_info
                .total_stake
                .checked_sub(block_info.total_children_stake)
                .unwrap()
                >= quorum
            {
                num_skipped += 1;
            }
        }

        // Basic sanity check to ensure the network meets fault tolerance assumption.
        if committed.len() > 1 {
            panic!(
                "Multiple committed blocks found for leader slot {}: {:?}",
                slot, committed
            );
        }
        // Under safety assumption, no other block can be committed in this slot.
        if committed.len() == 1 {
            return LeaderStatus::Commit(committed.pop().unwrap());
        }

        // When no block at the slot is undecided or committed, the whole slot is skipped.
        // Even if new blocks arrive at the slot later, they are guaranteed to be
        // skipped directly or indirectly.
        if num_skipped == num_blocks {
            return LeaderStatus::Skip(slot);
        }

        // There are undecided blocks and no committed block in the slot. So the slot is undecided.
        LeaderStatus::Undecided(slot)
    }

    /// Indirect-commit evaluation for every slot in `slots` (must all be at
    /// `decision_round`). BFS from `anchor_block` runs once and produces a
    /// per-block (authorities, stake) vote map; each requested slot is then
    /// scored against `certification_threshold` and returned as
    /// Commit(block) or Skip(slot). Never returns Undecided.
    ///
    /// Returns one LeaderStatus per `slots` entry, in the same order.
    pub(crate) fn try_indirect_decide(
        &self,
        anchor_block: &VerifiedBlock,
        decision_round_slots: &[Slot],
    ) -> Vec<LeaderStatus> {
        assert!(!decision_round_slots.is_empty());
        let decision_round = decision_round_slots[0].round;
        assert!(
            decision_round_slots
                .iter()
                .all(|s| s.round == decision_round)
        );

        // BFS once from the anchor: collect votes from decision_round+1 blocks
        // for blocks at decision_round.
        let mut to_visit = VecDeque::new();
        to_visit.push_back(anchor_block.clone());
        let mut visited = BTreeSet::new();
        visited.insert(anchor_block.reference());
        let mut decided_blocks: BTreeMap<BlockRef, (BTreeSet<AuthorityIndex>, Stake)> =
            BTreeMap::new();
        let dag_state = self.dag_state.read();
        while let Some(block) = to_visit.pop_front() {
            for ancestor in block.ancestors() {
                if ancestor.round < decision_round {
                    // This ancestor is irrelevant for decision round blocks.
                    continue;
                }
                // Always count direct (decision_round → decision_round+1)
                // edges, even when the ancestor was already enqueued for BFS
                // expansion via another voter.
                if ancestor.round == decision_round && block.round() == decision_round + 1 {
                    let entry = decided_blocks
                        .entry(*ancestor)
                        .or_insert((BTreeSet::new(), 0));
                    if entry.0.insert(block.author()) {
                        entry.1 += self.context.committee.stake(block.author());
                    }
                }
                if !visited.insert(*ancestor) {
                    // Already enqueued; don't re-walk its causal history.
                    continue;
                }
                let ancestor_block = dag_state
                    .get_block(ancestor)
                    .unwrap_or_else(|| panic!("Block {} must exist", ancestor));
                to_visit.push_back(ancestor_block);
            }
        }

        // Per-slot certification scan against the vote map.
        let cert_threshold = self.context.committee.certification_threshold();
        decision_round_slots
            .iter()
            .map(|slot| {
                let block_votes = decided_blocks.range(
                    BlockRef::new(slot.round, slot.authority, BlockDigest::MIN)
                        ..=BlockRef::new(slot.round, slot.authority, BlockDigest::MAX),
                );
                let mut certified_blocks = vec![];
                for (block_ref, (_authorities, voting_stake)) in block_votes {
                    if *voting_stake < cert_threshold {
                        continue;
                    }
                    let block = dag_state
                        .get_block(block_ref)
                        .unwrap_or_else(|| panic!("Block {} must exist", block_ref));
                    certified_blocks.push(block);
                }
                if certified_blocks.is_empty() {
                    return LeaderStatus::Skip(*slot);
                }
                if certified_blocks.len() > 1 {
                    // Multiple certified blocks at one slot is allowed under
                    // the Byzantine fault assumption: a Byzantine leader can
                    // produce equivocating blocks and honest voters may
                    // certify different ones. Only committing more than one
                    // would violate safety. Deterministic pick: first.
                    tracing::trace!(
                        "Picking first certified block for leader slot {} in indirect commit: {:?}",
                        slot,
                        certified_blocks
                    );
                }
                LeaderStatus::Commit(certified_blocks[0].clone())
            })
            .collect()
    }
}
