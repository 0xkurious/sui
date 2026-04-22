// Copyright (c) Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

use std::sync::Arc;

use consensus_config::AuthorityIndex;
use parking_lot::RwLock;

use crate::{
    block::{BlockAPI, Slot},
    commit::LeaderStatus,
    context::Context,
    dag_state::DagState,
    leader_slot_decider::LeaderSlotDecider,
    storage::mem_store::MemStore,
    test_dag::{build_dag, build_dag_layer},
};

fn setup(num_authorities: usize) -> (Arc<Context>, Arc<RwLock<DagState>>, LeaderSlotDecider) {
    let (mut context, _) = Context::new_for_test(num_authorities);
    context.protocol_config.set_enable_v3_for_testing(true);
    let context = Arc::new(context);
    let dag_state = Arc::new(RwLock::new(DagState::new(
        context.clone(),
        Arc::new(MemStore::new()),
    )));
    let decider = LeaderSlotDecider::new(context.clone(), dag_state.clone());
    (context, dag_state, decider)
}

/// A fully connected round 2 commits the round-1 leader directly.
#[tokio::test]
async fn try_direct_decide_commit() {
    telemetry_subscribers::init_for_testing();
    let (context, dag_state, decider) = setup(4);

    build_dag(context, dag_state, None, 2);

    let slot = Slot::new_for_test(1, 0);
    let status = decider.try_direct_decide(slot);

    match status {
        LeaderStatus::Commit(block) => {
            assert_eq!(block.author(), slot.authority);
            assert_eq!(block.round(), slot.round);
        }
        other => panic!("Expected Commit, got {other:?}"),
    }
}

/// When every round-2 block omits the round-1 leader, it is directly skipped.
#[tokio::test]
async fn try_direct_decide_skip() {
    telemetry_subscribers::init_for_testing();
    let (context, dag_state, decider) = setup(4);

    let refs_round_1 = build_dag(context.clone(), dag_state.clone(), None, 1);
    let refs_without_leader: Vec<_> = refs_round_1
        .into_iter()
        .filter(|r| r.author != AuthorityIndex::new_for_test(0))
        .collect();
    build_dag(context, dag_state, Some(refs_without_leader), 2);

    let slot = Slot::new_for_test(1, 0);
    let status = decider.try_direct_decide(slot);

    match status {
        LeaderStatus::Skip(s) => assert_eq!(s, slot),
        other => panic!("Expected Skip, got {other:?}"),
    }
}

/// Without a next round of blocks, no decision is possible — Undecided.
#[tokio::test]
async fn try_direct_decide_undecided_no_next_round() {
    telemetry_subscribers::init_for_testing();
    let (context, dag_state, decider) = setup(4);

    build_dag(context, dag_state, None, 1);

    let slot = Slot::new_for_test(1, 0);
    let status = decider.try_direct_decide(slot);

    match status {
        LeaderStatus::Undecided(s) => assert_eq!(s, slot),
        other => panic!("Expected Undecided, got {other:?}"),
    }
}

/// Round 2 has neither quorum-many votes for nor quorum-many blames against
/// the leader — direct decision must be Undecided.
#[tokio::test]
async fn try_direct_decide_undecided_split() {
    telemetry_subscribers::init_for_testing();
    let (context, dag_state, decider) = setup(4);

    let refs_round_1 = build_dag(context.clone(), dag_state.clone(), None, 1);
    let refs_without_leader: Vec<_> = refs_round_1
        .iter()
        .cloned()
        .filter(|r| r.author != AuthorityIndex::new_for_test(0))
        .collect();

    // 2 round-2 blocks reference all of round 1 (votes for leader); 2 omit
    // the leader. Total votes for leader = 2, against = 2 — neither side has
    // quorum (3) of stake.
    let mut authorities = context.committee.authorities();
    let mut connections = vec![];
    for _ in 0..2 {
        connections.push((authorities.next().unwrap().0, refs_round_1.clone()));
    }
    for _ in 0..2 {
        connections.push((authorities.next().unwrap().0, refs_without_leader.clone()));
    }
    build_dag_layer(connections, dag_state.clone());

    let slot = Slot::new_for_test(1, 0);
    let status = decider.try_direct_decide(slot);

    match status {
        LeaderStatus::Undecided(s) => assert_eq!(s, slot),
        other => panic!("Expected Undecided, got {other:?}"),
    }
}

/// An anchor whose causal history includes the round-1 leader certifies it
/// indirectly — we must commit.
#[tokio::test]
async fn try_indirect_decide_commit() {
    telemetry_subscribers::init_for_testing();
    let (context, dag_state, decider) = setup(4);

    // Fully connected DAG up to round 4. Round 2 has all 4 round-1 blocks as
    // ancestors, so the round-1 leader is certified. Anchor is any round-4
    // block.
    let refs_round_4 = build_dag(context.clone(), dag_state.clone(), None, 4);
    let anchor = dag_state.read().get_block(&refs_round_4[0]).unwrap();

    let slot = Slot::new_for_test(1, 0);
    let statuses = decider.try_indirect_decide(&anchor, &[slot]);

    assert_eq!(statuses.len(), 1);
    match statuses.into_iter().next().unwrap() {
        LeaderStatus::Commit(block) => {
            assert_eq!(block.author(), slot.authority);
            assert_eq!(block.round(), slot.round);
        }
        other => panic!("Expected Commit, got {other:?}"),
    }
}

/// No round-2 block references the round-1 leader, so the anchor's causal
/// history does not certify it — Skip.
#[tokio::test]
async fn try_indirect_decide_skip() {
    telemetry_subscribers::init_for_testing();
    let (context, dag_state, decider) = setup(4);

    let refs_round_1 = build_dag(context.clone(), dag_state.clone(), None, 1);
    let refs_without_leader: Vec<_> = refs_round_1
        .into_iter()
        .filter(|r| r.author != AuthorityIndex::new_for_test(0))
        .collect();
    let refs_round_2 = build_dag(
        context.clone(),
        dag_state.clone(),
        Some(refs_without_leader),
        2,
    );
    let refs_round_3 = build_dag(context.clone(), dag_state.clone(), Some(refs_round_2), 3);

    let anchor = dag_state.read().get_block(&refs_round_3[0]).unwrap();

    let slot = Slot::new_for_test(1, 0);
    let statuses = decider.try_indirect_decide(&anchor, &[slot]);

    assert_eq!(statuses.len(), 1);
    match statuses.into_iter().next().unwrap() {
        LeaderStatus::Skip(s) => assert_eq!(s, slot),
        other => panic!("Expected Skip, got {other:?}"),
    }
}

/// One call covers multiple slots at the same decision round; each slot is
/// decided independently against the same BFS-collected vote map.
#[tokio::test]
async fn try_indirect_decide_multi_slot() {
    telemetry_subscribers::init_for_testing();
    let (context, dag_state, decider) = setup(4);

    // Round 1 fully. Round 2 omits authority-0's round-1 block but includes
    // authorities 1..3. Round 3 fully on top.
    let refs_round_1 = build_dag(context.clone(), dag_state.clone(), None, 1);
    let refs_without_leader_0: Vec<_> = refs_round_1
        .iter()
        .cloned()
        .filter(|r| r.author != AuthorityIndex::new_for_test(0))
        .collect();
    let refs_round_2 = build_dag(
        context.clone(),
        dag_state.clone(),
        Some(refs_without_leader_0),
        2,
    );
    let refs_round_3 = build_dag(context.clone(), dag_state.clone(), Some(refs_round_2), 3);

    let anchor = dag_state.read().get_block(&refs_round_3[0]).unwrap();

    let slots = vec![Slot::new_for_test(1, 0), Slot::new_for_test(1, 1)];
    let statuses = decider.try_indirect_decide(&anchor, &slots);

    assert_eq!(statuses.len(), 2);
    let mut iter = statuses.into_iter();
    // (1, 0) — never referenced → Skip.
    match iter.next().unwrap() {
        LeaderStatus::Skip(s) => assert_eq!(s, slots[0]),
        other => panic!("Expected Skip(1,0), got {other:?}"),
    }
    // (1, 1) — referenced by all 4 round-2 blocks → Commit.
    match iter.next().unwrap() {
        LeaderStatus::Commit(block) => {
            assert_eq!(block.author(), slots[1].authority);
            assert_eq!(block.round(), slots[1].round);
        }
        other => panic!("Expected Commit(1,1), got {other:?}"),
    }
}
