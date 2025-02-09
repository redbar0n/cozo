/*
 * Copyright 2022, The Cozo Project Authors.
 *
 * This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0.
 * If a copy of the MPL was not distributed with this file,
 * You can obtain one at https://mozilla.org/MPL/2.0/.
 */

use std::sync::atomic::{AtomicU32, AtomicU64};
use std::sync::Arc;

use miette::{bail, Result};

use crate::data::tuple::TupleT;
use crate::data::value::DataValue;
use crate::runtime::relation::RelationId;
use crate::storage::temp::TempTx;
use crate::storage::StoreTx;

pub struct SessionTx<'a> {
    pub(crate) store_tx: Box<dyn StoreTx<'a> + 'a>,
    pub(crate) temp_store_tx: TempTx,
    pub(crate) relation_store_id: Arc<AtomicU64>,
    pub(crate) temp_store_id: AtomicU32,
}

pub const CURRENT_STORAGE_VERSION: [u8; 1] = [0x00];

fn storage_version_key() -> Vec<u8> {
    let storage_version_tuple = vec![DataValue::Null, DataValue::from("STORAGE_VERSION")];
    storage_version_tuple.encode_as_key(RelationId::SYSTEM)
}

impl<'a> SessionTx<'a> {
    pub(crate) fn init_storage(&mut self) -> Result<RelationId> {
        let tuple = vec![DataValue::Null];
        let t_encoded = tuple.encode_as_key(RelationId::SYSTEM);
        let found = self.store_tx.get(&t_encoded, false)?;
        let storage_version_key = storage_version_key();
        let ret = match found {
            None => {
                self.store_tx
                    .put(&storage_version_key, &CURRENT_STORAGE_VERSION)?;
                self.store_tx
                    .put(&t_encoded, &RelationId::new(0).raw_encode())?;
                RelationId::SYSTEM
            }
            Some(slice) => {
                let version_found = self.store_tx.get(&storage_version_key, false)?;
                match version_found {
                    None => {
                        bail!("Storage is used but un-versioned, probably created by an ancient version of Cozo.")
                    }
                    Some(v) => {
                        if &v != &CURRENT_STORAGE_VERSION {
                            bail!(
                                "Version mismatch: expect storage version {:?}, got {:?}",
                                CURRENT_STORAGE_VERSION,
                                v
                            )
                        }
                    }
                }
                RelationId::raw_decode(&slice)
            }
        };
        Ok(ret)
    }

    pub fn commit_tx(&mut self) -> Result<()> {
        self.store_tx.commit()?;
        Ok(())
    }
}
