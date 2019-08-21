use crate::types::Eth1DataFetcher;
use parking_lot::RwLock;
use std::collections::BTreeMap;
use std::sync::Arc;
use types::*;
use web3::futures::Future;
use web3::types::*;

/// Cache for recent Eth1Data fetched from the Eth1 chain.
#[derive(Clone, Debug)]
pub struct Eth1DataCache<F: Eth1DataFetcher> {
    cache: Arc<RwLock<BTreeMap<U256, Eth1Data>>>,
    last_block: Arc<RwLock<u64>>,
    fetcher: F,
}

impl<F: Eth1DataFetcher + 'static> Eth1DataCache<F> {
    pub fn new(fetcher: F) -> Self {
        Eth1DataCache {
            cache: Arc::new(RwLock::new(BTreeMap::new())),
            // Should ideally start from block where Eth1 chain starts accepting deposits.
            last_block: Arc::new(RwLock::new(0)),
            fetcher,
        }
    }

    /// Called periodically to populate the cache with Eth1Data from most recent blocks.
    pub fn update_cache(&self) -> Box<dyn Future<Item = (), Error = ()>> {
        // Make tasks and communicate between them using channels
        let cache_updated = self.cache.clone();
        let last_block = self.last_block.clone();
        let fetcher = self.fetcher.clone();
        Box::new(
            self.fetcher
                .get_current_block_number()
                .and_then(move |current_block_number: U256| {
                    let last_block_read: u64 = *last_block.read();
                    for i in last_block_read..current_block_number.as_u64() {
                        let cache_new = cache_updated.clone();
                        if !cache_new.read().contains_key(&U256::from(i)) {
                            let eth1_future = fetch_eth1_data(i, current_block_number, &fetcher);
                            eth1_future.and_then(move |data| {
                                let mut eth1_cache = cache_new.write();
                                let data = data.unwrap();
                                eth1_cache.insert(data.0, data.1);
                                Ok(())
                            });
                            let mut last_block = *last_block.write();
                            last_block = current_block_number.as_u64();
                            // TODO: Delete older stuff in a fifo order.
                        }
                    }
                    Ok(())
                })
                .map_err(|_| println!("Update cache failed")),
        )
    }

    // /// Get `Eth1Data` object at a distance of `distance` from the perceived head of the currrent Eth1 chain.
    // /// Returns the object from the cache if present, else fetches from Eth1Fetcher.
    // pub fn get_eth1_data(&mut self, distance: u64) -> Option<Eth1Data> {
    //     let current_block_number: U256 = self.fetcher.get_current_block_number().wait().ok()?;
    //     let block_number: U256 = current_block_number.checked_sub(distance.into())?;
    //     if self.cache.contains_key(&block_number) {
    //         return Some(self.cache.get(&block_number)?.clone());
    //     } else {
    //         if let Some((block_number, eth1_data)) =
    //             self.fetch_eth1_data(distance, current_block_number)
    //         {
    //             self.cache.insert(block_number, eth1_data);
    //             return Some(self.cache.get(&block_number)?.clone());
    //         }
    //     }
    //     None
    // }

    // /// Returns a Vec<Eth1Data> corresponding to given distance range.
    // pub fn get_eth1_data_in_range(&mut self, start: u64, end: u64) -> Vec<Eth1Data> {
    //     (start..end)
    //         .map(|h| self.get_eth1_data(h))
    //         .flatten() // Chuck None values
    //         .collect::<Vec<Eth1Data>>()
    // }
}

/// Fetches Eth1 data from the Eth1Data fetcher object.
pub fn fetch_eth1_data<F: Eth1DataFetcher>(
    distance: u64,
    current_block_number: U256,
    fetcher: &F,
) -> impl Future<Item = Option<(U256, Eth1Data)>, Error = ()> {
    let block_number: U256 = current_block_number
        .checked_sub(distance.into())
        .unwrap_or(U256::zero());
    let deposit_root = fetcher.get_deposit_root(Some(BlockNumber::Number(block_number.as_u64())));
    let deposit_count = fetcher.get_deposit_count(Some(BlockNumber::Number(block_number.as_u64())));
    let block_hash = fetcher.get_block_hash_by_height(block_number.as_u64());
    let eth1_data_future = deposit_root.join3(deposit_count, block_hash);
    eth1_data_future.map(move |data| {
        let eth1_data = Eth1Data {
            deposit_root: data.0,
            deposit_count: data.1?,
            block_hash: data.2?,
        };
        Some((block_number, eth1_data))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ContractConfig;
    use crate::web3_fetcher::Web3DataFetcher;
    use std::time::{Duration, Instant};
    use tokio::timer::Delay;
    use web3::types::Address;

    // Note: Running tests using ganache-cli instance with config
    // from https://github.com/ChainSafe/lodestar#starting-private-eth1-chain

    fn setup() -> Web3DataFetcher {
        let deposit_contract_address: Address =
            "8c594691C0E592FFA21F153a16aE41db5beFcaaa".parse().unwrap();
        let deposit_contract = ContractConfig {
            address: deposit_contract_address,
            abi: include_bytes!("deposit_contract.json").to_vec(),
        };
        let w3 = Web3DataFetcher::new("ws://localhost:8545", deposit_contract);
        return w3;
    }

    #[test]
    fn test_fetch() {
        let w3 = setup();
        // let cache = Eth1DataCache::new(Arc::new(w3));
        let when = Instant::now() + Duration::from_millis(5000);
        let task1 = Delay::new(when)
            .and_then(|_| {
                println!("Hello world!");
                Ok(())
            })
            .map_err(|e| panic!("delay errored; err={:?}", e));
        tokio::run(task1);
        let task2 = fetch_eth1_data(0, 10.into(), &w3).and_then(|data| {
            println!("{:?}", data);
            Ok(())
        });
        tokio::run(task2);
    }
}
