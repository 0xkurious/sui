// Copyright (c) Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

use std::sync::Arc;

use async_graphql::Context;
use tokio::sync::broadcast;
use tracing::warn;

use crate::api::scalars::uint53::UInt53;
use crate::api::types::checkpoint::CCheckpoint;
use crate::api::types::checkpoint::Checkpoint;
use crate::api::types::event::Event;
use crate::api::types::event::filter::EventFilter;
use crate::api::types::transaction::Transaction;
use crate::api::types::transaction::filter::TransactionFilter;
use crate::config::Limits;
use crate::config::SubscriptionConfig;
use crate::error::RpcError;
use crate::error::bad_user_input;
use crate::scope::Scope;
use crate::task::streaming::CheckpointBroadcaster;
use crate::task::streaming::StreamingPackageStore;
use crate::task::streaming::SubscriptionResources;
use crate::task::streaming::scan_checkpoints;

#[derive(thiserror::Error, Debug, Clone)]
pub(crate) enum Error {
    #[error("At most one of `afterCursor` or `afterCheckpoint` can be specified")]
    OneResumeArg,
}

#[derive(Default)]
pub struct Subscription;

#[async_graphql::Subscription]
impl Subscription {
    /// Subscribe to checkpoints as they are finalized.
    ///
    /// Pass `afterCursor` (opaque) or `afterCheckpoint` (sequence number) to resume from a known point. The two arguments are mutually exclusive.
    ///
    /// This subscription is not yet available for use.
    async fn checkpoints(
        &self,
        ctx: &Context<'_>,
        after_cursor: Option<CCheckpoint>,
        after_checkpoint: Option<UInt53>,
    ) -> Result<impl futures::Stream<Item = Result<Checkpoint, RpcError>>, RpcError<Error>> {
        if after_cursor.is_some() && after_checkpoint.is_some() {
            return Err(bad_user_input(Error::OneResumeArg));
        }
        let resume_from: Option<u64> = after_cursor
            .map(|c| *c)
            .or_else(|| after_checkpoint.map(u64::from));

        let package_store = ctx.data::<Arc<StreamingPackageStore>>()?.clone();
        let limits: &Limits = ctx.data()?;
        let resolver_limits = limits.package_resolver();
        let config = ctx.data::<SubscriptionConfig>()?;
        let resume_batch_size = config.resume_batch_size;
        let resume_transition_threshold = config.resume_transition_threshold;
        let resume_max_recovery_attempts = config.resume_max_recovery_attempts;
        let resources = ctx.data::<SubscriptionResources>()?;
        let mut receiver = resources.broadcaster.resubscribe();
        let fetcher = Arc::new(resources.ledger_grpc_reader.clone());
        let network_tip = resources.network_tip.clone();

        Ok(async_stream::stream! {
            let mut last_yielded: Option<u64> = resume_from;
            let mut recovery_attempts = 0u32;

            'recovery: loop {
                // Phase 1: scan via LedgerService to cover the gap to the live tip.
                if let Some(start_after) = last_yielded {
                    for await item in scan_checkpoints(
                        fetcher.clone(),
                        network_tip.clone(),
                        start_after,
                        resume_batch_size,
                        resume_transition_threshold,
                    ) {
                        let processed = item?;
                        last_yielded = Some(processed.sequence_number);
                        let scope = Scope::for_streamed_checkpoint(
                            package_store.clone(),
                            resolver_limits.clone(),
                            processed.clone(),
                        );
                        yield Ok(Checkpoint {
                            sequence_number: processed.sequence_number,
                            scope,
                            streamed_data: Some(processed),
                        });
                    }
                }

                // Phase 2: Transition to live.
                loop {
                    match receiver.recv().await {
                        Ok(processed) => match last_yielded {
                            // Receiver still has buffered items older than our scan position.
                            Some(ly) if processed.sequence_number <= ly => continue,
                            // Live advanced past where the scan ended (or Lag skipped items).
                            Some(ly) if processed.sequence_number > ly + 1 => {
                                recovery_attempts += 1;
                                if recovery_attempts >= resume_max_recovery_attempts {
                                    yield Err(anyhow::anyhow!(
                                        "Failed to catch up to the live tip after \
                                         {resume_max_recovery_attempts} recovery attempts; \
                                         please reconnect.",
                                    )
                                    .into());
                                    return;
                                }
                                continue 'recovery;
                            }
                            _ => {
                                recovery_attempts = 0;
                                last_yielded = Some(processed.sequence_number);
                                let scope = Scope::for_streamed_checkpoint(
                                    package_store.clone(),
                                    resolver_limits.clone(),
                                    processed.clone(),
                                );
                                yield Ok(Checkpoint {
                                    sequence_number: processed.sequence_number,
                                    scope,
                                    streamed_data: Some(processed),
                                });
                            }
                        },
                        Err(e) => {
                            // Recover from transient Lagged events; everything else propagates.
                            if let broadcast::error::RecvError::Lagged(n) = &e {
                                recovery_attempts += 1;
                                if recovery_attempts < resume_max_recovery_attempts {
                                    warn!(
                                        missed = n,
                                        attempt = recovery_attempts,
                                        "Lagged; recovering via LedgerService",
                                    );
                                    continue 'recovery;
                                }
                            }
                            yield Err(broadcast_error(e));
                            return;
                        }
                    }
                }
            }
        })
    }

    /// Subscribe to transactions as they are finalized, with optional filtering.
    ///
    /// Each matching transaction is yielded individually as it appears in finalized
    /// checkpoints. Transactions are ordered by checkpoint, then by position within
    /// the checkpoint.
    ///
    /// This subscription is not yet available for use.
    async fn transactions(
        &self,
        ctx: &Context<'_>,
        filter: Option<TransactionFilter>,
    ) -> Result<impl futures::Stream<Item = Result<Transaction, RpcError>>, RpcError> {
        let package_store = ctx.data::<Arc<StreamingPackageStore>>()?.clone();
        let limits: &Limits = ctx.data()?;
        let resolver_limits = limits.package_resolver();
        let mut receiver = ctx.data::<CheckpointBroadcaster>()?.resubscribe();
        let filter = filter.unwrap_or_default();

        Ok(async_stream::stream! {
            loop {
                match receiver.recv().await {
                    Ok(processed) => {
                        let scope = Scope::for_streamed_checkpoint(
                            package_store.clone(),
                            resolver_limits.clone(),
                            processed.clone(),
                        );
                        // TODO(DVX-2050): Pre-filter checkpoints using bloom filters
                        // before evaluating exact matches, to skip checkpoints with
                        // no matching transactions.
                        for tx in &processed.transactions {
                            if !filter.matches(&tx.contents) {
                                continue;
                            }
                            yield Transaction::with_contents(scope.clone(), tx.contents.clone());
                        }
                    }
                    Err(e) => {
                        yield Err(broadcast_error(e));
                        break;
                    }
                }
            }
        })
    }

    /// Subscribe to events as they are emitted, with optional filtering.
    ///
    /// Each matching event is yielded individually as it appears in finalized
    /// checkpoints. Events are ordered by checkpoint, then by transaction
    /// position within the checkpoint, then by position within the transaction.
    ///
    /// This subscription is not yet available for use.
    async fn events(
        &self,
        ctx: &Context<'_>,
        filter: Option<EventFilter>,
    ) -> Result<impl futures::Stream<Item = Result<Event, RpcError>>, RpcError> {
        let package_store = ctx.data::<Arc<StreamingPackageStore>>()?.clone();
        let limits: &Limits = ctx.data()?;
        let resolver_limits = limits.package_resolver();
        let mut receiver = ctx.data::<CheckpointBroadcaster>()?.resubscribe();
        let filter = filter.unwrap_or_default();

        Ok(async_stream::stream! {
            loop {
                match receiver.recv().await {
                    Ok(processed) => {
                        let timestamp_ms = Some(processed.summary.timestamp_ms);
                        let scope = Scope::for_streamed_checkpoint(
                            package_store.clone(),
                            resolver_limits.clone(),
                            processed.clone(),
                        );
                        for tx in &processed.transactions {
                            let events = tx.contents.events().unwrap_or_default();
                            for (idx, native) in events.into_iter().enumerate() {
                                if !filter.matches(&native) {
                                    continue;
                                }
                                yield Ok(Event {
                                    scope: scope.with_active_transaction_contents(
                                        tx.digest,
                                        tx.contents.clone(),
                                    ),
                                    native,
                                    transaction_digest: tx.digest,
                                    sequence_number: idx as u64,
                                    timestamp_ms,
                                });
                            }
                        }
                    }
                    Err(e) => {
                        yield Err(broadcast_error(e));
                        break;
                    }
                }
            }
        })
    }
}

fn broadcast_error(e: broadcast::error::RecvError) -> RpcError {
    match e {
        broadcast::error::RecvError::Lagged(missed_count) => {
            warn!(missed_count, "Subscription lagged, disconnecting");
            anyhow::anyhow!(
                "Subscription too slow: missed {missed_count} checkpoints. \
                 Please reconnect and use the query API to backfill \
                 from your last seen sequenceNumber."
            )
            .into()
        }
        broadcast::error::RecvError::Closed => {
            warn!("Checkpoint broadcast channel closed");
            anyhow::anyhow!("Checkpoint stream has been shut down. Please reconnect.").into()
        }
    }
}
