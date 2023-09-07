//! Common CLI utility functions.

use boyer_moore_magiclen::BMByte;
use eyre::Result;
use reth_consensus_common::validation::validate_block_standalone;
use reth_db::{
    cursor::DbCursorRO,
    database::Database,
    table::{Decode, Table, TableRow},
    transaction::{DbTx, DbTxMut},
    DatabaseError, RawKey, RawTable, TableRawRow,
};
use reth_interfaces::p2p::{
    bodies::client::BodiesClient,
    headers::client::{HeadersClient, HeadersRequest},
    priority::Priority,
};
use reth_primitives::{
    fs, BlockHashOrNumber, ChainSpec, HeadersDirection, SealedBlock, SealedHeader,
};
use std::{
    env::VarError,
    path::{Path, PathBuf},
    rc::Rc,
    sync::Arc,
};
use tracing::info;

/// Get a single header from network
pub async fn get_single_header<Client>(
    client: Client,
    id: BlockHashOrNumber,
) -> Result<SealedHeader>
where
    Client: HeadersClient,
{
    let request = HeadersRequest { direction: HeadersDirection::Rising, limit: 1, start: id };

    let (peer_id, response) =
        client.get_headers_with_priority(request, Priority::High).await?.split();

    if response.len() != 1 {
        client.report_bad_message(peer_id);
        eyre::bail!("Invalid number of headers received. Expected: 1. Received: {}", response.len())
    }

    let header = response.into_iter().next().unwrap().seal_slow();

    let valid = match id {
        BlockHashOrNumber::Hash(hash) => header.hash() == hash,
        BlockHashOrNumber::Number(number) => header.number == number,
    };

    if !valid {
        client.report_bad_message(peer_id);
        eyre::bail!(
            "Received invalid header. Received: {:?}. Expected: {:?}",
            header.num_hash(),
            id
        );
    }

    Ok(header)
}

/// Get a body from network based on header
pub async fn get_single_body<Client>(
    client: Client,
    chain_spec: Arc<ChainSpec>,
    header: SealedHeader,
) -> Result<SealedBlock>
where
    Client: BodiesClient,
{
    let (peer_id, response) = client.get_block_body(header.hash).await?.split();

    if response.is_none() {
        client.report_bad_message(peer_id);
        eyre::bail!("Invalid number of bodies received. Expected: 1. Received: 0")
    }

    let block = response.unwrap();
    let block = SealedBlock {
        header,
        body: block.transactions,
        ommers: block.ommers,
        withdrawals: block.withdrawals,
    };

    validate_block_standalone(&block, &chain_spec)?;

    Ok(block)
}

/// Wrapper over DB that implements many useful DB queries.
pub struct DbTool<'a, DB: Database> {
    pub(crate) db: &'a DB,
    pub(crate) chain: Arc<ChainSpec>,
}

impl<'a, DB: Database> DbTool<'a, DB> {
    /// Takes a DB where the tables have already been created.
    pub(crate) fn new(db: &'a DB, chain: Arc<ChainSpec>) -> eyre::Result<Self> {
        Ok(Self { db, chain })
    }

    /// Grabs the contents of the table within a certain index range and places the
    /// entries into a [`HashMap`][std::collections::HashMap].
    ///
    /// [`ListFilter`] can be used to further
    /// filter down the desired results. (eg. List only rows which include `0xd3adbeef`)
    pub fn list<T: Table>(&self, filter: &ListFilter<T>) -> Result<(Vec<TableRow<T>>, usize)> {
        let bmb = Rc::new(filter.search.as_ref().and_then(BMByte::from));
        if bmb.is_none() && filter.has_search() {
            eyre::bail!("Invalid search.")
        }

        let mut hits = 0;

        let data = self.db.view(|tx| {
            let mut cursor =
                tx.cursor_read::<RawTable<T>>().expect("Was not able to obtain a cursor.");

            let map_filter = |row: Result<TableRawRow<T>, _>| {
                if let Ok((k, v)) = row {
                    let result = || {
                        if filter.only_count {
                            return None
                        }
                        Some((k.key().unwrap(), v.value().unwrap()))
                    };
                    match &*bmb {
                        Some(searcher) => {
                            if searcher.find_first_in(v.raw_value()).is_some() ||
                                searcher.find_first_in(k.raw_key()).is_some()
                            {
                                hits += 1;
                                return result()
                            }
                        }
                        None => {
                            hits += 1;
                            return result()
                        }
                    }
                }
                None
            };

            let seek_key = filter.seek_key.clone().map(|key| RawKey::new(key));

            if filter.reverse {
                Ok(cursor
                    .walk_back(seek_key)?
                    .skip(filter.skip)
                    .filter_map(map_filter)
                    .take(filter.len)
                    .collect::<Vec<(_, _)>>())
            } else {
                Ok(cursor
                    .walk(seek_key)?
                    .skip(filter.skip)
                    .filter_map(map_filter)
                    .take(filter.len)
                    .collect::<Vec<(_, _)>>())
            }
        })?;

        Ok((data.map_err(|e: DatabaseError| eyre::eyre!(e))?, hits))
    }

    /// Grabs the content of the table for the given key
    pub fn get<T: Table>(&self, key: T::Key) -> Result<Option<T::Value>> {
        self.db.view(|tx| tx.get::<T>(key))?.map_err(|e| eyre::eyre!(e))
    }

    /// Drops the database at the given path.
    pub fn drop(&mut self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        info!(target: "reth::cli", "Dropping database at {:?}", path);
        fs::remove_dir_all(path)?;
        Ok(())
    }

    /// Drops the provided table from the database.
    pub fn drop_table<T: Table>(&mut self) -> Result<()> {
        self.db.update(|tx| tx.clear::<T>())??;
        Ok(())
    }
}

/// Parses a user-specified path with support for environment variables and common shorthands (e.g.
/// ~ for the user's home directory).
pub fn parse_path(value: &str) -> Result<PathBuf, shellexpand::LookupError<VarError>> {
    shellexpand::full(value).map(|path| PathBuf::from(path.into_owned()))
}

/// Filters the results coming from the database.
#[derive(Debug)]
pub struct ListFilter<T: Table> {
    /// Skip first N entries.
    pub skip: usize,
    /// Take N entries.
    pub len: usize,
    /// Sequence of bytes that will be searched on values and keys from the database.
    pub search: Option<Vec<u8>>,
    /// Reverse order of entries.
    pub reverse: bool,
    /// Only counts the number of filtered entries without decoding and returning them.
    pub only_count: bool,
    pub seek_key: Option<T::Key>,
}

impl<T: Table> ListFilter<T> {
    /// Creates a new [`ListFilter`].
    pub fn new(
        skip: usize,
        len: usize,
        search: Option<Vec<u8>>,
        reverse: bool,
        only_count: bool,
        seek_key: Option<T::Key>,
    ) -> Self {
        ListFilter { skip, len, search, reverse, only_count, seek_key }
    }

    /// If `search` is not [`Option::None`] and has a list of bytes,
    /// then filter for rows that have this sequence.
    pub fn has_search(&self) -> bool {
        self.search.as_ref().map_or(false, |search| !search.is_empty())
    }

    /// If `seek` is not [`Option::None`], then seek the cursor to the row whose key is greater than
    /// or equal to this sequence.
    pub fn has_seek(&self) -> bool {
        self.seek_key.is_none()
    }

    /// Updates the page with new `skip` and `len` values.
    pub fn update_page(&mut self, skip: usize, len: usize) {
        self.skip = skip;
        self.len = len;
    }
}
