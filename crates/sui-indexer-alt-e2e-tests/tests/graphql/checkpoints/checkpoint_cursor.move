// Copyright (c) Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

//# init --protocol-version 108 --accounts A --simulator

//# create-checkpoint

//# run-graphql
{ # Each checkpoint exposes an opaque cursor derived from its sequence number.
  c0: checkpoint(sequenceNumber: 0) { sequenceNumber cursor }
  c1: checkpoint(sequenceNumber: 1) { sequenceNumber cursor }
}
