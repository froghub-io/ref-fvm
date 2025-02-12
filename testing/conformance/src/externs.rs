use anyhow::anyhow;
// Copyright 2021-2023 Protocol Labs
// SPDX-License-Identifier: Apache-2.0, MIT
use fvm::externs::{Chain, Consensus, Externs, Rand};
use fvm_shared::clock::ChainEpoch;
use fvm_shared::consensus::ConsensusFault;

use crate::rand::ReplayingRand;
use crate::vector::{Randomness, TipsetCid};

/// The externs stub for testing. Forwards randomness requests to the randomness
/// replayer, which replays randomness stored in the vector.
pub struct TestExterns {
    pub tipset_cids: Vec<TipsetCid>,
    rand: ReplayingRand,
}

impl TestExterns {
    /// Creates a new TestExterns from randomness contained in a vector.
    pub fn new(r: &Randomness) -> Self {
        TestExterns {
            tipset_cids: Default::default(),
            rand: ReplayingRand::new(r.as_slice()),
        }
    }
}

impl Externs for TestExterns {}

impl Rand for TestExterns {
    fn get_chain_randomness(
        &self,
        pers: i64,
        round: ChainEpoch,
        entropy: &[u8],
    ) -> anyhow::Result<[u8; 32]> {
        self.rand.get_chain_randomness(pers, round, entropy)
    }

    fn get_beacon_randomness(
        &self,
        pers: i64,
        round: ChainEpoch,
        entropy: &[u8],
    ) -> anyhow::Result<[u8; 32]> {
        self.rand.get_beacon_randomness(pers, round, entropy)
    }
}

impl Consensus for TestExterns {
    fn verify_consensus_fault(
        &self,
        _h1: &[u8],
        _h2: &[u8],
        _extra: &[u8],
    ) -> anyhow::Result<(Option<ConsensusFault>, i64)> {
        todo!()
    }
}

impl Chain for TestExterns {
    fn get_tipset_cid(&self, _epoch: ChainEpoch) -> anyhow::Result<cid::Cid> {
        for tipset in &self.tipset_cids {
            if tipset.epoch == _epoch {
                return Ok(tipset.cid);
            }
        }
        Err(anyhow!("cannot find tipset cid, epoch {}", _epoch))
    }
}
