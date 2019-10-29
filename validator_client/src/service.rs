/// The Validator Client service.
///
/// Connects to a beacon node and negotiates the correct chain id.
///
/// Once connected, the service loads known validators keypairs from disk. Every slot,
/// the service pings the beacon node, asking for new duties for each of the validators.
///
/// When a validator needs to either produce a block or sign an attestation, it requests the
/// data from the beacon node and performs the signing before publishing the block to the beacon
/// node.
use crate::attestation_producer::{
    AttestationProducer, AttestationRestClient, BeaconNodeAttestation,
};
use crate::block_producer::{BeaconBlockRestClient, BeaconNodeBlock, BlockProducer};
use crate::config::Config as ValidatorConfig;
use crate::duties::{BeaconNodeDuties, DutiesManager, EpochDutiesMap, ValidatorServiceRestClient};
use crate::error as error_chain;
use crate::rest_client::RestClient;
use crate::signer::Signer;
use bls::Keypair;
use eth2_config::Eth2Config;
use futures::future::{loop_fn, Loop};
use slog::{crit, info, trace, warn};
use slot_clock::{SlotClock, SystemTimeSlotClock};
use std::marker::PhantomData;
use std::sync::Arc;
use std::sync::RwLock;
use std::time::{Duration, Instant};
use tokio::prelude::*;
use tokio::runtime::Builder;
use tokio::timer::Interval;
use tokio_timer::clock::Clock;
use types::{ChainSpec, Epoch, EthSpec, Fork, Slot};

/// A type for returning a future of whatever object we're playing with
/// (usually BeaconBlock or Attestation)
pub type BoxFut<T, E> = Box<dyn Future<Item = T, Error = E> + Send>;

/// A fixed amount of time after a slot to perform operations. This gives the node time to complete
/// per-slot processes.
const TIME_DELAY_FROM_SLOT: Duration = Duration::from_millis(100);

/// The validator service. This is the main thread that executes and maintains validator
/// duties.
//TODO: Generalize the BeaconNode types to use testing
pub struct Service<
    D: BeaconNodeDuties + 'static,
    S: Signer + 'static,
    E: EthSpec,
    B: BeaconNodeBlock,
    A: BeaconNodeAttestation,
> {
    /// The node's current fork version we are processing on.
    fork: Fork,
    /// The slot clock for this service.
    slot_clock: SystemTimeSlotClock,
    /// The slot that is currently, or was previously processed by the service.
    current_slot: Option<Slot>,
    slots_per_epoch: u64,
    /// The chain specification for this clients instance.
    spec: Arc<ChainSpec>,
    /// The duties manager which maintains the state of when to perform actions.
    duties_manager: Arc<DutiesManager<D, S>>,
    // GRPC Clients
    /// The beacon block GRPC client.
    beacon_block_client: Arc<B>,
    /// The attester GRPC client.
    attestation_client: Arc<A>,
    /// The validator client logger.
    log: slog::Logger,
    _phantom: PhantomData<E>,
}

impl<
        D: BeaconNodeDuties + 'static,
        S: Signer + 'static,
        E: EthSpec,
        B: BeaconNodeBlock,
        A: BeaconNodeAttestation,
    > Service<D, S, E, B, A>
{
    ///  Initial connection to the beacon node to determine its properties.
    ///
    ///  This tries to connect to a beacon node. Once connected, it initialised the REST clients
    ///  and returns an instance of the service.
    fn initialize_service(
        validator_config: ValidatorConfig,
        eth2_config: Eth2Config,
        log: slog::Logger,
    ) -> error_chain::Result<Service<D, Keypair, E, B, A>> {
        let server_url = format!(
            "{}:{}",
            validator_config.server, validator_config.server_port
        );

        let rest_client = RestClient::new(validator_config.clone())?;

        let try_info_continuously = loop_fn((log, rest_client), |(log, r_client)| {
            r_client
                .make_get_request_with_timeout("/node/info", Vec::new())
                .then(|result| match result {
                    Ok(r) => {
                        info!(log, "Connected to Beacon Node");
                        Ok(Loop::Break(r))
                    }
                    Err(e) => {
                        warn!(log, "Unable to connect to Beacon Node, trying again.");
                        Ok(Loop::Continue(r_client))
                    }
                })
        });

        let info_response = try_info_continuously.wait()?;
        info!(log,
            "Connected to Beacon Node";
            "version" => info_response,
        );
        /*

        {
                Err(e) => {
                    let retry_seconds = 5;
                    warn!(
                        log,
                        "Could not connect to beacon node";
                        "error" => format!("{:?}", e),
                        "retry_in" => format!("{} seconds", retry_seconds),
                    );
                    std::thread::sleep(Duration::from_secs(retry_seconds));
                    continue;
                }
                Ok(info) => {
                    // verify the node's network id
                    if eth2_config.spec.network_id != info.network_id as u8 {
                        error!(
                            log,
                            "Beacon Node's genesis time is in the future. No work to do.\n Exiting"
                        );
                        return Err(format!("Beacon node has the wrong chain id. Expected chain id: {}, node's chain id: {}", eth2_config.spec.network_id, info.network_id).into());
                    }
                    break info;
                }
            };
        };

        info!(
            log,
            "Beacon node connected via gRPC";
            "version" => node_info.version.clone(),
            "network_id" => node_info.network_id,
            "genesis_time" => genesis_time
        );

        // initialize the RPC clients

        // Beacon node gRPC beacon block endpoints.
        let beacon_block_client = {
            let ch = ChannelBuilder::new(env.clone()).connect(&server_url);
            let beacon_block_service_client = Arc::new(BeaconBlockServiceClient::new(ch));
            // a wrapper around the service client to implement the beacon block node trait
            Arc::new(BeaconBlockGrpcClient::new(beacon_block_service_client))
        };

        // Beacon node gRPC validator endpoints.
        let validator_client = {
            let ch = ChannelBuilder::new(env.clone()).connect(&server_url);
            Arc::new(ValidatorServiceClient::new(ch))
        };

        //Beacon node gRPC attester endpoints.
        let attestation_client = {
            let ch = ChannelBuilder::new(env.clone()).connect(&server_url);
            Arc::new(AttestationServiceClient::new(ch))
        };
        */

        // Load generated keypairs
        let keypairs = Arc::new(validator_config.fetch_keys(&log)?);

        // Builds a mapping of Epoch -> Map(PublicKey, EpochDuty)
        // where EpochDuty contains slot numbers and attestation data that each validator needs to
        // produce work on.
        let duties_map = RwLock::new(EpochDutiesMap::new(E::slots_per_epoch()));

        let duties_client = Arc::new(ValidatorServiceRestClient {
            endpoint: "/beacon/validator/duties".into(),
            client: RestClient::new(validator_config.clone()),
        });

        // builds a manager which maintains the list of current duties for all known validators
        // and can check when a validator needs to perform a task.
        let duties_manager = Arc::new(DutiesManager {
            duties_map,
            // these are abstract objects capable of signing
            signers: keypairs,
            beacon_node: duties_client,
        });

        /*
        // build requisite objects to form Self
        let genesis_time = node_info.get_genesis_time();
        let genesis_slot = Slot::from(node_info.get_genesis_slot());
        let spec = Arc::new(eth2_config.spec);
        */

        // TODO: keypairs are randomly generated; they should be loaded from a file or generated.
        // https://github.com/sigp/lighthouse/issues/160
        //let keypairs = Arc::new(generate_deterministic_keypairs(8));

        /*
        let proto_fork = node_info.get_fork();
        previous_version.copy_from_slice(&proto_fork.get_previous_version()[..4]);
        current_version.copy_from_slice(&proto_fork.get_current_version()[..4]);
        */
        let mut previous_version: [u8; 4] = [0; 4];
        let mut current_version: [u8; 4] = [0; 4];
        let fork = Fork {
            previous_version,
            current_version,
            epoch: Epoch::new(0),
        };

        let beacon_block_client = Arc::new(BeaconBlockRestClient {
            endpoint: "/beacon/validator/block".into(),
            client: RestClient::new(validator_config.clone()),
        });
        let attestation_client = Arc::new(AttestationRestClient {
            endpoint: "/beacon/validator/attestation".into(),
            client: RestClient::new(validator_config.clone()),
        });

        Ok(Service {
            fork,
            slot_clock: SystemTimeSlotClock::new(
                Slot::Duration::from_secs(0),
                Duration::from_millis(eth2_config.spec.milliseconds_per_slot),
            ),
            current_slot: None,
            slots_per_epoch: E::slots_per_epoch(),
            spec: Arc::new(eth2_config.spec),
            duties_manager,
            beacon_block_client,
            attestation_client,
            log,
            _phantom: PhantomData,
        })
    }

    /// Initialise the service then run the core thread.
    // TODO: Improve handling of generic BeaconNode types, to stub grpcClient
    pub fn start(
        client_config: ValidatorConfig,
        eth2_config: Eth2Config,
        log: slog::Logger,
    ) -> error_chain::Result<()> {
        // connect to the node and retrieve its properties and initialize the clients
        let mut service =
            Service::<D, S, E, B, A>::initialize_service(client_config, eth2_config, log.clone())?;

        // we have connected to a node and established its parameters. Spin up the core service

        // set up the validator service runtime
        let mut runtime = Builder::new()
            .clock(Clock::system())
            .name_prefix("validator-client-")
            .build()
            .map_err(|e| format!("Tokio runtime failed: {}", e))?;

        let duration_to_next_slot = service
            .slot_clock
            .duration_to_next_slot()
            .ok_or_else::<error_chain::Error, _>(|| {
                "Unable to determine duration to next slot. Exiting.".into()
            })?;

        // set up the validator work interval - start at next slot and proceed every slot
        let interval = {
            // Set the interval to start at the next slot, and every slot after
            let slot_duration = Duration::from_millis(service.spec.milliseconds_per_slot);
            //TODO: Handle checked add correctly
            Interval::new(Instant::now() + duration_to_next_slot, slot_duration)
        };

        if service.slot_clock.now().is_none() {
            warn!(
                log,
                "Starting node prior to genesis";
            );
        }

        info!(
            log,
            "Waiting for next slot";
            "seconds_to_wait" => duration_to_next_slot.as_secs()
        );

        /* kick off the core service */
        runtime.block_on(
            interval
                .for_each(move |_| {
                    // wait for node to process
                    std::thread::sleep(TIME_DELAY_FROM_SLOT);
                    // if a non-fatal error occurs, proceed to the next slot.
                    let _ignore_error = service.per_slot_execution();
                    // completed a slot process
                    Ok(())
                })
                .map_err(|e| format!("Service thread failed: {:?}", e)),
        )?;
        // validator client exited
        Ok(())
    }

    /// The execution logic that runs every slot.
    // Errors are logged to output, and core execution continues unless fatal errors occur.
    fn per_slot_execution(&mut self) -> error_chain::Result<()> {
        /* get the new current slot and epoch */
        self.update_current_slot()?;

        /* check for new duties */
        self.check_for_duties();

        /* process any required duties for validators */
        self.process_duties();

        trace!(
            self.log,
            "Per slot execution finished";
        );

        Ok(())
    }

    /// Updates the known current slot and epoch.
    fn update_current_slot(&mut self) -> error_chain::Result<()> {
        let wall_clock_slot = self
            .slot_clock
            .now()
            .ok_or_else::<error_chain::Error, _>(|| {
                "Genesis is not in the past. Exiting.".into()
            })?;

        let wall_clock_epoch = wall_clock_slot.epoch(self.slots_per_epoch);

        // this is a non-fatal error. If the slot clock repeats, the node could
        // have been slow to process the previous slot and is now duplicating tasks.
        // We ignore duplicated but raise a critical error.
        if let Some(current_slot) = self.current_slot {
            if wall_clock_slot <= current_slot {
                crit!(
                    self.log,
                    "The validator tried to duplicate a slot. Likely missed the previous slot"
                );
                return Err("Duplicate slot".into());
            }
        }
        self.current_slot = Some(wall_clock_slot);
        info!(self.log, "Processing"; "slot" => wall_clock_slot.as_u64(), "epoch" => wall_clock_epoch.as_u64());
        Ok(())
    }

    /// For all known validator keypairs, update any known duties from the beacon node.
    fn check_for_duties(&mut self) {
        let cloned_manager = self.duties_manager.clone();
        let cloned_log = self.log.clone();
        let current_epoch = self
            .current_slot
            .expect("The current slot must be updated before checking for duties")
            .epoch(self.slots_per_epoch);

        trace!(
            self.log,
            "Checking for duties";
            "epoch" => current_epoch
        );

        // spawn a new thread separate to the runtime
        // TODO: Handle thread termination/timeout
        // TODO: Add duties thread back in, with channel to process duties in duty change.
        // leave sequential for now.
        //std::thread::spawn(move || {
        // the return value is a future which returns ready.
        // built to be compatible with the tokio runtime.
        let _empty = cloned_manager.run_update(current_epoch, cloned_log.clone());
        //});
    }

    /// If there are any duties to process, spawn a separate thread and perform required actions.
    fn process_duties(&mut self) {
        if let Some(work) = self.duties_manager.get_current_work(
            self.current_slot
                .expect("The current slot must be updated before processing duties"),
        ) {
            trace!(
                self.log,
                "Processing duties";
                "work_items" => work.len()
            );

            for (signer_index, work_type) in work {
                if work_type.produce_block {
                    // we need to produce a block
                    // spawns a thread to produce a beacon block
                    let signers = self.duties_manager.signers.clone(); // this is an arc
                    let fork = self.fork.clone();
                    let slot = self
                        .current_slot
                        .expect("The current slot must be updated before processing duties");
                    let spec = self.spec.clone();
                    let beacon_node = self.beacon_block_client.clone();
                    let log = self.log.clone();
                    let slots_per_epoch = self.slots_per_epoch;
                    std::thread::spawn(move || {
                        info!(
                            log,
                            "Producing a block";
                            "validator"=> format!("{}", signers[signer_index]),
                            "slot"=> slot
                        );
                        let signer = &signers[signer_index];
                        let mut block_producer = BlockProducer {
                            fork,
                            slot,
                            spec,
                            beacon_node,
                            signer,
                            slots_per_epoch,
                            _phantom: PhantomData::<E>,
                            log,
                        };
                        block_producer.handle_produce_block();
                    });
                }
                if work_type.attestation_duty.is_some() {
                    // we need to produce an attestation
                    // spawns a thread to produce and sign an attestation
                    let slot = self
                        .current_slot
                        .expect("The current slot must be updated before processing duties");
                    let signers = self.duties_manager.signers.clone(); // this is an arc
                    let fork = self.fork.clone();
                    let spec = self.spec.clone();
                    let beacon_node = self.attestation_client.clone();
                    let log = self.log.clone();
                    let slots_per_epoch = self.slots_per_epoch;
                    std::thread::spawn(move || {
                        info!(
                            log,
                            "Producing an attestation";
                            "validator"=> format!("{}", signers[signer_index]),
                            "slot"=> slot
                        );
                        let signer = &signers[signer_index];
                        let mut attestation_producer = AttestationProducer {
                            fork,
                            duty: work_type.attestation_duty.expect("Should never be none"),
                            spec,
                            beacon_node,
                            signer,
                            slots_per_epoch,
                            _phantom: PhantomData::<E>,
                        };
                        attestation_producer.handle_produce_attestation(log);
                    });
                }
            }
        }
    }
}
