use sled::Batch;

use crate::{
    consensus2::{util::Timestamp, Block},
    util::serial::{deserialize, serialize},
    Result,
};

const SLED_BLOCK_TREE: &[u8] = b"_blocks";

pub struct BlockStore(sled::Tree);

impl BlockStore {
    /// Opens a new or existing `BlockStore` on the given sled database.
    pub fn new(db: &sled::Db, genesis_ts: Timestamp, genesis_data: blake3::Hash) -> Result<Self> {
        let tree = db.open_tree(SLED_BLOCK_TREE)?;
        let store = Self(tree);

        // In case the store is empty, create the genesis block.
        if store.0.is_empty() {
            store.insert(&[Block::genesis_block(genesis_ts, genesis_data)])?;
        }

        Ok(store)
    }

    /// Insert a slice of [`Block`] into the blockstore. With sled, the
    /// operation is done as a batch.
    /// The blocks are hashed with BLAKE3 and this blockhash is used as
    /// the key, while value is the serialized block itself.
    pub fn insert(&self, blocks: &[Block]) -> Result<Vec<blake3::Hash>> {
        let mut ret = Vec::with_capacity(blocks.len());
        let mut batch = Batch::default();
        for i in blocks {
            let serialized = serialize(i);
            let blockhash = blake3::hash(&serialized);
            batch.insert(blockhash.as_bytes(), serialized);
            ret.push(blockhash);
        }

        self.0.apply_batch(batch)?;
        Ok(ret)
    }

    /// Fetch given blockhashes from the blockstore.
    /// The resulting vector contains `Option` which is `Some` if the block
    /// was found in the blockstore, and `None`, if it has not.
    pub fn get(&self, blockhashes: &[blake3::Hash]) -> Result<Vec<Option<Block>>> {
        let mut ret: Vec<Option<Block>> = Vec::with_capacity(blockhashes.len());

        for i in blockhashes {
            if let Some(found) = self.0.get(i.as_bytes())? {
                let block = deserialize(&found)?;
                ret.push(Some(block));
            } else {
                ret.push(None);
            }
        }

        Ok(ret)
    }

    /// Check if the blockstore contains a given blockhash.
    pub fn contains(&self, blockhash: blake3::Hash) -> Result<bool> {
        Ok(self.0.contains_key(blockhash.as_bytes())?)
    }

    /*
    /// Fetch the first block in the tree, based on the Ord implementation for Vec<u8>.
    pub fn get_first(&self) -> Result<Option<(blake3::Hash, Block)>> {
        if let Some(found) = self.0.first()? {
            let hash_bytes: [u8; 32] = found.0.as_ref().try_into().unwrap();
            let block = deserialize(&found.1)?;
            return Ok(Some((hash_bytes.into(), block)))
        }

        Ok(None)
    }

    /// Fetch the last block in the tree, based on the Ord implementation for Vec<u8>.
    pub fn get_last(&self) -> Result<Option<(blake3::Hash, Block)>> {
        if let Some(found) = self.0.last()? {
            let hash_bytes: [u8; 32] = found.0.as_ref().try_into().unwrap();
            let block = deserialize(&found.1)?;
            return Ok(Some((hash_bytes.into(), block)))
        }

        Ok(None)
    }

    /// Fetch the block and its hash before the provided blockhash, if one exists.
    pub fn get_lt(&self, blockhash: blake3::Hash) -> Result<Option<(blake3::Hash, Block)>> {
        if let Some(found) = self.0.get_lt(blockhash.as_bytes())? {
            let hash_bytes: [u8; 32] = found.0.as_ref().try_into().unwrap();
            let block = deserialize(&found.1)?;
            return Ok(Some((hash_bytes.into(), block)))
        }

        Ok(None)
    }

    /// Fetch the block and its hash after the provided blockhash, if one exists.
    pub fn get_gt(&self, blockhash: blake3::Hash) -> Result<Option<(blake3::Hash, Block)>> {
        if let Some(found) = self.0.get_gt(blockhash.as_bytes())? {
            let hash_bytes: [u8; 32] = found.0.as_ref().try_into().unwrap();
            let block = deserialize(&found.1)?;
            return Ok(Some((hash_bytes.into(), block)))
        }

        Ok(None)
    }

    /// Retrieve an iterator over a range of blockhashes.
    /// When iterating, take care of potential memory limitations if you're
    /// storing results in memory. For blockchain sync, it should probably
    /// be done in chunks.
    // Usage:
    // ```
    // let mut r = get_range(foo, bar);
    // while let Some((k, v)) = r.next() {
    //     let hash_bytes: [u8; 32] = k.as_ref().try_into().unwrap();
    //     let block = deserialize(&v)?;
    // }
    // ```
    pub fn get_range(&self, start: blake3::Hash, end: blake3::Hash) -> sled::Iter {
        let start: &[u8] = start.as_bytes().as_ref();
        let end: &[u8] = end.as_bytes().as_ref();

        self.0.range(start..end)
    }
    */
}
