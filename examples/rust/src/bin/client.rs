use {
    anyhow::Context,
    backoff::{future::retry, ExponentialBackoff},
    clap::{Parser, Subcommand, ValueEnum},
    futures::{future::TryFutureExt, sink::SinkExt, stream::StreamExt},
    log::{error, info},
    serde_json::{json, Value},
    solana_sdk::{hash::Hash, pubkey::Pubkey, signature::Signature},
    solana_transaction_status::UiTransactionEncoding,
    std::{collections::HashMap, env, fs::File, sync::Arc, time::Duration},
    tokio::sync::Mutex,
    tonic::transport::channel::ClientTlsConfig,
    yellowstone_grpc_client::{GeyserGrpcClient, GeyserGrpcClientError, Interceptor},
    yellowstone_grpc_proto::{
        convert_from,
        prelude::{
            subscribe_request_filter_accounts_filter::Filter as AccountsFilterOneof,
            subscribe_request_filter_accounts_filter_lamports::Cmp as AccountsFilterLamports,
            subscribe_request_filter_accounts_filter_memcmp::Data as AccountsFilterMemcmpOneof,
            subscribe_update::UpdateOneof, CommitmentLevel, SubscribeRequest,
            SubscribeRequestAccountsDataSlice, SubscribeRequestFilterAccounts,
            SubscribeRequestFilterAccountsFilter, SubscribeRequestFilterAccountsFilterLamports,
            SubscribeRequestFilterAccountsFilterMemcmp, SubscribeRequestFilterBlocks,
            SubscribeRequestFilterBlocksMeta, SubscribeRequestFilterEntry,
            SubscribeRequestFilterSlots, SubscribeRequestFilterTransactions, SubscribeRequestPing,
            SubscribeUpdate, SubscribeUpdateAccountInfo, SubscribeUpdateEntry,
            SubscribeUpdateTransactionInfo,
        },
    },
};

type SlotsFilterMap = HashMap<String, SubscribeRequestFilterSlots>;
type AccountFilterMap = HashMap<String, SubscribeRequestFilterAccounts>;
type TransactionsFilterMap = HashMap<String, SubscribeRequestFilterTransactions>;
type TransactionsStatusFilterMap = HashMap<String, SubscribeRequestFilterTransactions>;
type EntryFilterMap = HashMap<String, SubscribeRequestFilterEntry>;
type BlocksFilterMap = HashMap<String, SubscribeRequestFilterBlocks>;
type BlocksMetaFilterMap = HashMap<String, SubscribeRequestFilterBlocksMeta>;

#[derive(Debug, Clone, Parser)]
#[clap(author, version, about)]
struct Args {
    #[clap(short, long, default_value_t = String::from("http://127.0.0.1:10000"))]
    /// Service endpoint
    endpoint: String,

    #[clap(long)]
    x_token: Option<String>,

    /// Commitment level: processed, confirmed or finalized
    #[clap(long)]
    commitment: Option<ArgsCommitment>,

    #[command(subcommand)]
    action: Action,
}

impl Args {
    fn get_commitment(&self) -> Option<CommitmentLevel> {
        Some(self.commitment.unwrap_or_default().into())
    }

    async fn connect(&self) -> anyhow::Result<GeyserGrpcClient<impl Interceptor>> {
        GeyserGrpcClient::build_from_shared(self.endpoint.clone())?
            .x_token(self.x_token.clone())?
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(10))
            .tls_config(ClientTlsConfig::new().with_native_roots())?
            .connect()
            .await
            .map_err(Into::into)
    }
}

#[derive(Debug, Clone, Copy, Default, ValueEnum)]
enum ArgsCommitment {
    #[default]
    Processed,
    Confirmed,
    Finalized,
}

impl From<ArgsCommitment> for CommitmentLevel {
    fn from(commitment: ArgsCommitment) -> Self {
        match commitment {
            ArgsCommitment::Processed => CommitmentLevel::Processed,
            ArgsCommitment::Confirmed => CommitmentLevel::Confirmed,
            ArgsCommitment::Finalized => CommitmentLevel::Finalized,
        }
    }
}

#[derive(Debug, Clone, Subcommand)]
enum Action {
    HealthCheck,
    HealthWatch,
    Subscribe(Box<ActionSubscribe>),
    Ping {
        #[clap(long, short, default_value_t = 0)]
        count: i32,
    },
    GetLatestBlockhash,
    GetBlockHeight,
    GetSlot,
    IsBlockhashValid {
        #[clap(long, short)]
        blockhash: String,
    },
    GetVersion,
}

#[derive(Debug, Clone, clap::Args)]
struct ActionSubscribe {
    /// Subscribe on accounts updates
    #[clap(long)]
    accounts: bool,

    /// Filter by presence of field txn_signature
    accounts_nonempty_txn_signature: Option<bool>,

    /// Filter by Account Pubkey
    #[clap(long)]
    accounts_account: Vec<String>,

    /// Path to a JSON array of account addresses
    #[clap(long)]
    accounts_account_path: Option<String>,

    /// Filter by Owner Pubkey
    #[clap(long)]
    accounts_owner: Vec<String>,

    /// Filter by Offset and Data, format: `offset,data in base58`
    #[clap(long)]
    accounts_memcmp: Vec<String>,

    /// Filter by Data size
    #[clap(long)]
    accounts_datasize: Option<u64>,

    /// Filter valid token accounts
    #[clap(long)]
    accounts_token_account_state: bool,

    /// Filter by lamports, format: `eq:42` / `ne:42` / `lt:42` / `gt:42`
    #[clap(long)]
    accounts_lamports: Vec<String>,

    /// Receive only part of updated data account, format: `offset,size`
    #[clap(long)]
    accounts_data_slice: Vec<String>,

    /// Subscribe on slots updates
    #[clap(long)]
    slots: bool,

    /// Filter slots by commitment
    #[clap(long)]
    slots_filter_by_commitment: bool,

    /// Subscribe on transactions updates
    #[clap(long)]
    transactions: bool,

    /// Filter vote transactions
    #[clap(long)]
    transactions_vote: Option<bool>,

    /// Filter failed transactions
    #[clap(long)]
    transactions_failed: Option<bool>,

    /// Filter by transaction signature
    #[clap(long)]
    transactions_signature: Option<String>,

    /// Filter included account in transactions
    #[clap(long)]
    transactions_account_include: Vec<String>,

    /// Filter excluded account in transactions
    #[clap(long)]
    transactions_account_exclude: Vec<String>,

    /// Filter required account in transactions
    #[clap(long)]
    transactions_account_required: Vec<String>,

    /// Subscribe on transactions_status updates
    #[clap(long)]
    transactions_status: bool,

    /// Filter vote transactions for transactions_status
    #[clap(long)]
    transactions_status_vote: Option<bool>,

    /// Filter failed transactions for transactions_status
    #[clap(long)]
    transactions_status_failed: Option<bool>,

    /// Filter by transaction signature for transactions_status
    #[clap(long)]
    transactions_status_signature: Option<String>,

    /// Filter included account in transactions for transactions_status
    #[clap(long)]
    transactions_status_account_include: Vec<String>,

    /// Filter excluded account in transactions for transactions_status
    #[clap(long)]
    transactions_status_account_exclude: Vec<String>,

    /// Filter required account in transactions for transactions_status
    #[clap(long)]
    transactions_status_account_required: Vec<String>,

    #[clap(long)]
    entry: bool,

    /// Subscribe on block updates
    #[clap(long)]
    blocks: bool,

    /// Filter included account in transactions
    #[clap(long)]
    blocks_account_include: Vec<String>,

    /// Include transactions to block message
    #[clap(long)]
    blocks_include_transactions: Option<bool>,

    /// Include accounts to block message
    #[clap(long)]
    blocks_include_accounts: Option<bool>,

    /// Include entries to block message
    #[clap(long)]
    blocks_include_entries: Option<bool>,

    /// Subscribe on block meta updates (without transactions)
    #[clap(long)]
    blocks_meta: bool,

    /// Send ping in subscribe request
    #[clap(long)]
    ping: Option<i32>,

    /// Resubscribe (only to slots) after
    #[clap(long)]
    resub: Option<usize>,
}

impl Action {
    async fn get_subscribe_request(
        &self,
        commitment: Option<CommitmentLevel>,
    ) -> anyhow::Result<Option<(SubscribeRequest, usize)>> {
        Ok(match self {
            Self::Subscribe(args) => {
                let mut accounts: AccountFilterMap = HashMap::new();
                if args.accounts {
                    let mut accounts_account = args.accounts_account.clone();
                    if let Some(path) = args.accounts_account_path.clone() {
                        let accounts = tokio::task::block_in_place(move || {
                            let file = File::open(path)?;
                            Ok::<Vec<String>, anyhow::Error>(serde_json::from_reader(file)?)
                        })?;
                        accounts_account.extend(accounts);
                    }

                    let mut filters = vec![];
                    for filter in args.accounts_memcmp.iter() {
                        match filter.split_once(',') {
                            Some((offset, data)) => {
                                filters.push(SubscribeRequestFilterAccountsFilter {
                                    filter: Some(AccountsFilterOneof::Memcmp(
                                        SubscribeRequestFilterAccountsFilterMemcmp {
                                            offset: offset
                                                .parse()
                                                .map_err(|_| anyhow::anyhow!("invalid offset"))?,
                                            data: Some(AccountsFilterMemcmpOneof::Base58(
                                                data.trim().to_string(),
                                            )),
                                        },
                                    )),
                                });
                            }
                            _ => anyhow::bail!("invalid memcmp"),
                        }
                    }
                    if let Some(datasize) = args.accounts_datasize {
                        filters.push(SubscribeRequestFilterAccountsFilter {
                            filter: Some(AccountsFilterOneof::Datasize(datasize)),
                        });
                    }
                    if args.accounts_token_account_state {
                        filters.push(SubscribeRequestFilterAccountsFilter {
                            filter: Some(AccountsFilterOneof::TokenAccountState(true)),
                        });
                    }
                    for filter in args.accounts_lamports.iter() {
                        match filter.split_once(':') {
                            Some((cmp, value)) => {
                                let Ok(value) = value.parse() else {
                                    anyhow::bail!("invalid lamports value: {value}");
                                };
                                filters.push(SubscribeRequestFilterAccountsFilter {
                                    filter: Some(AccountsFilterOneof::Lamports(
                                        SubscribeRequestFilterAccountsFilterLamports {
                                            cmp: Some(match cmp {
                                                "eq" => AccountsFilterLamports::Eq(value),
                                                "ne" => AccountsFilterLamports::Ne(value),
                                                "lt" => AccountsFilterLamports::Lt(value),
                                                "gt" => AccountsFilterLamports::Gt(value),
                                                _ => {
                                                    anyhow::bail!("invalid lamports filter: {cmp}")
                                                }
                                            }),
                                        },
                                    )),
                                });
                            }
                            _ => anyhow::bail!("invalid lamports"),
                        }
                    }

                    accounts.insert(
                        "client".to_owned(),
                        SubscribeRequestFilterAccounts {
                            nonempty_txn_signature: args.accounts_nonempty_txn_signature,
                            account: accounts_account,
                            owner: args.accounts_owner.clone(),
                            filters,
                        },
                    );
                }

                let mut slots: SlotsFilterMap = HashMap::new();
                if args.slots {
                    slots.insert(
                        "client".to_owned(),
                        SubscribeRequestFilterSlots {
                            filter_by_commitment: Some(args.slots_filter_by_commitment),
                        },
                    );
                }

                let mut transactions: TransactionsFilterMap = HashMap::new();
                if args.transactions {
                    transactions.insert(
                        "client".to_string(),
                        SubscribeRequestFilterTransactions {
                            vote: args.transactions_vote,
                            failed: args.transactions_failed,
                            signature: args.transactions_signature.clone(),
                            account_include: args.transactions_account_include.clone(),
                            account_exclude: args.transactions_account_exclude.clone(),
                            account_required: args.transactions_account_required.clone(),
                        },
                    );
                }

                let mut transactions_status: TransactionsStatusFilterMap = HashMap::new();
                if args.transactions_status {
                    transactions_status.insert(
                        "client".to_string(),
                        SubscribeRequestFilterTransactions {
                            vote: args.transactions_status_vote,
                            failed: args.transactions_status_failed,
                            signature: args.transactions_status_signature.clone(),
                            account_include: args.transactions_status_account_include.clone(),
                            account_exclude: args.transactions_status_account_exclude.clone(),
                            account_required: args.transactions_status_account_required.clone(),
                        },
                    );
                }

                let mut entry: EntryFilterMap = HashMap::new();
                if args.entry {
                    entry.insert("client".to_owned(), SubscribeRequestFilterEntry {});
                }

                let mut blocks: BlocksFilterMap = HashMap::new();
                if args.blocks {
                    blocks.insert(
                        "client".to_owned(),
                        SubscribeRequestFilterBlocks {
                            account_include: args.blocks_account_include.clone(),
                            include_transactions: args.blocks_include_transactions,
                            include_accounts: args.blocks_include_accounts,
                            include_entries: args.blocks_include_entries,
                        },
                    );
                }

                let mut blocks_meta: BlocksMetaFilterMap = HashMap::new();
                if args.blocks_meta {
                    blocks_meta.insert("client".to_owned(), SubscribeRequestFilterBlocksMeta {});
                }

                let mut accounts_data_slice = Vec::new();
                for data_slice in args.accounts_data_slice.iter() {
                    match data_slice.split_once(',') {
                        Some((offset, length)) => match (offset.parse(), length.parse()) {
                            (Ok(offset), Ok(length)) => {
                                accounts_data_slice
                                    .push(SubscribeRequestAccountsDataSlice { offset, length });
                            }
                            _ => anyhow::bail!("invalid data_slice"),
                        },
                        _ => anyhow::bail!("invalid data_slice"),
                    }
                }

                let ping = args.ping.map(|id| SubscribeRequestPing { id });

                Some((
                    SubscribeRequest {
                        slots,
                        accounts,
                        transactions,
                        transactions_status,
                        entry,
                        blocks,
                        blocks_meta,
                        commitment: commitment.map(|x| x as i32),
                        accounts_data_slice,
                        ping,
                    },
                    args.resub.unwrap_or(0),
                ))
            }
            _ => None,
        })
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env::set_var(
        env_logger::DEFAULT_FILTER_ENV,
        env::var_os(env_logger::DEFAULT_FILTER_ENV).unwrap_or_else(|| "info".into()),
    );
    env_logger::init();

    let args = Args::parse();
    let zero_attempts = Arc::new(Mutex::new(true));

    // The default exponential backoff strategy intervals:
    // [500ms, 750ms, 1.125s, 1.6875s, 2.53125s, 3.796875s, 5.6953125s,
    // 8.5s, 12.8s, 19.2s, 28.8s, 43.2s, 64.8s, 97s, ... ]
    retry(ExponentialBackoff::default(), move || {
        let args = args.clone();
        let zero_attempts = Arc::clone(&zero_attempts);

        async move {
            let mut zero_attempts = zero_attempts.lock().await;
            if *zero_attempts {
                *zero_attempts = false;
            } else {
                info!("Retry to connect to the server");
            }
            drop(zero_attempts);

            let commitment = args.get_commitment();
            let mut client = args.connect().await.map_err(backoff::Error::transient)?;
            info!("Connected");

            match &args.action {
                Action::HealthCheck => client
                    .health_check()
                    .await
                    .map_err(anyhow::Error::new)
                    .map(|response| info!("response: {response:?}")),
                Action::HealthWatch => geyser_health_watch(client).await,
                Action::Subscribe(_) => {
                    let (request, resub) = args
                        .action
                        .get_subscribe_request(commitment)
                        .await
                        .map_err(backoff::Error::Permanent)?
                        .ok_or(backoff::Error::Permanent(anyhow::anyhow!(
                            "expect subscribe action"
                        )))?;

                    geyser_subscribe(client, request, resub).await
                }
                Action::Ping { count } => client
                    .ping(*count)
                    .await
                    .map_err(anyhow::Error::new)
                    .map(|response| info!("response: {response:?}")),
                Action::GetLatestBlockhash => client
                    .get_latest_blockhash(commitment)
                    .await
                    .map_err(anyhow::Error::new)
                    .map(|response| info!("response: {response:?}")),
                Action::GetBlockHeight => client
                    .get_block_height(commitment)
                    .await
                    .map_err(anyhow::Error::new)
                    .map(|response| info!("response: {response:?}")),
                Action::GetSlot => client
                    .get_slot(commitment)
                    .await
                    .map_err(anyhow::Error::new)
                    .map(|response| info!("response: {response:?}")),
                Action::IsBlockhashValid { blockhash } => client
                    .is_blockhash_valid(blockhash.clone(), commitment)
                    .await
                    .map_err(anyhow::Error::new)
                    .map(|response| info!("response: {response:?}")),
                Action::GetVersion => client
                    .get_version()
                    .await
                    .map_err(anyhow::Error::new)
                    .map(|response| info!("response: {response:?}")),
            }
            .map_err(backoff::Error::transient)?;

            Ok::<(), backoff::Error<anyhow::Error>>(())
        }
        .inspect_err(|error| error!("failed to connect: {error}"))
    })
    .await
    .map_err(Into::into)
}

async fn geyser_health_watch(mut client: GeyserGrpcClient<impl Interceptor>) -> anyhow::Result<()> {
    let mut stream = client.health_watch().await?;
    info!("stream opened");
    while let Some(message) = stream.next().await {
        info!("new message: {message:?}");
    }
    info!("stream closed");
    Ok(())
}

async fn geyser_subscribe(
    mut client: GeyserGrpcClient<impl Interceptor>,
    request: SubscribeRequest,
    resub: usize,
) -> anyhow::Result<()> {
    let (mut subscribe_tx, mut stream) = client.subscribe_with_request(Some(request)).await?;

    info!("stream opened");
    let mut counter = 0;
    while let Some(message) = stream.next().await {
        match message {
            Ok(SubscribeUpdate {
                filters,
                update_oneof,
            }) => {
                match update_oneof {
                    Some(UpdateOneof::Account(msg)) => {
                        let account = msg
                            .account
                            .ok_or(anyhow::anyhow!("no account in the message"))?;
                        let mut value = create_pretty_account(account)?;
                        value["isStartup"] = json!(msg.is_startup);
                        value["slot"] = json!(msg.slot);
                        print_update("account", &filters, value);
                    }
                    Some(UpdateOneof::Slot(msg)) => {
                        let status = CommitmentLevel::try_from(msg.status)
                            .context("failed to decode commitment")?;
                        print_update(
                            "slot",
                            &filters,
                            json!({
                                "slot": msg.slot,
                                "parent": msg.parent,
                                "status": status.as_str_name()
                            }),
                        );
                    }
                    Some(UpdateOneof::Transaction(msg)) => {
                        let tx = msg
                            .transaction
                            .ok_or(anyhow::anyhow!("no transaction in the message"))?;
                        let mut value = create_pretty_transaction(tx)?;
                        value["slot"] = json!(msg.slot);
                        print_update("transaction", &filters, value);
                    }
                    Some(UpdateOneof::TransactionStatus(msg)) => {
                        print_update(
                            "transactionStatus",
                            &filters,
                            json!({
                                "slot": msg.slot,
                                "signature": Signature::try_from(msg.signature.as_slice()).context("invalid signature")?.to_string(),
                                "isVote": msg.is_vote,
                                "index": msg.index,
                                "err": convert_from::create_tx_error(msg.err.as_ref())
                                    .map_err(|error| anyhow::anyhow!(error))
                                    .context("invalid error")?,
                            }),
                        );
                    }
                    Some(UpdateOneof::Entry(msg)) => {
                        print_update("entry", &filters, create_pretty_entry(msg)?);
                    }
                    Some(UpdateOneof::BlockMeta(msg)) => {
                        print_update(
                            "blockmeta",
                            &filters,
                            json!({
                                "slot": msg.slot,
                                "blockhash": msg.blockhash,
                                "rewards": if let Some(rewards) = msg.rewards {
                                    Some(convert_from::create_rewards_obj(rewards).map_err(|error| anyhow::anyhow!(error))?)
                                } else {
                                    None
                                },
                                "blockTime": msg.block_time.map(|obj| obj.timestamp),
                                "blockHeight": msg.block_height.map(|obj| obj.block_height),
                                "parentSlot": msg.parent_slot,
                                "parentBlockhash": msg.parent_blockhash,
                                "executedTransactionCount": msg.executed_transaction_count,
                                "entriesCount": msg.entries_count,
                            }),
                        );
                    }
                    Some(UpdateOneof::Block(msg)) => {
                        print_update(
                            "block",
                            &filters,
                            json!({
                                "slot": msg.slot,
                                "blockhash": msg.blockhash,
                                "rewards": if let Some(rewards) = msg.rewards {
                                    Some(convert_from::create_rewards_obj(rewards).map_err(|error| anyhow::anyhow!(error))?)
                                } else {
                                    None
                                },
                                "blockTime": msg.block_time.map(|obj| obj.timestamp),
                                "blockHeight": msg.block_height.map(|obj| obj.block_height),
                                "parentSlot": msg.parent_slot,
                                "parentBlockhash": msg.parent_blockhash,
                                "executedTransactionCount": msg.executed_transaction_count,
                                "transactions": msg.transactions.into_iter().map(create_pretty_transaction).collect::<Result<Value, _>>()?,
                                "updatedAccountCount": msg.updated_account_count,
                                "accounts": msg.accounts.into_iter().map(create_pretty_account).collect::<Result<Value, _>>()?,
                                "entriesCount": msg.entries_count,
                                "entries": msg.entries.into_iter().map(create_pretty_entry).collect::<Result<Value, _>>()?,
                            }),
                        );
                    }
                    Some(UpdateOneof::Ping(_)) => {
                        // This is necessary to keep load balancers that expect client pings alive. If your load balancer doesn't
                        // require periodic client pings then this is unnecessary
                        subscribe_tx
                            .send(SubscribeRequest {
                                ping: Some(SubscribeRequestPing { id: 1 }),
                                ..Default::default()
                            })
                            .await?;
                    }
                    Some(UpdateOneof::Pong(_)) => {}
                    None => {
                        error!("update not found in the message");
                        break;
                    }
                }
            }
            Err(error) => {
                error!("error: {error:?}");
                break;
            }
        }

        // Example to illustrate how to resubscribe/update the subscription
        counter += 1;
        if counter == resub {
            let mut new_slots: SlotsFilterMap = HashMap::new();
            new_slots.insert("client".to_owned(), SubscribeRequestFilterSlots::default());

            subscribe_tx
                .send(SubscribeRequest {
                    slots: new_slots.clone(),
                    accounts: HashMap::default(),
                    transactions: HashMap::default(),
                    transactions_status: HashMap::default(),
                    entry: HashMap::default(),
                    blocks: HashMap::default(),
                    blocks_meta: HashMap::default(),
                    commitment: None,
                    accounts_data_slice: Vec::default(),
                    ping: None,
                })
                .await
                .map_err(GeyserGrpcClientError::SubscribeSendError)?;
        }
    }
    info!("stream closed");
    Ok(())
}

fn create_pretty_account(account: SubscribeUpdateAccountInfo) -> anyhow::Result<Value> {
    Ok(json!({
        "pubkey": Pubkey::try_from(account.pubkey).map_err(|_| anyhow::anyhow!("invalid account pubkey"))?.to_string(),
        "lamports": account.lamports,
        "owner": Pubkey::try_from(account.owner).map_err(|_| anyhow::anyhow!("invalid account owner"))?.to_string(),
        "executable": account.executable,
        "rentEpoch": account.rent_epoch,
        "data": hex::encode(account.data),
        "writeVersion": account.write_version,
        "txnSignature": account.txn_signature.map(|sig| bs58::encode(sig).into_string()),
    }))
}

fn create_pretty_transaction(tx: SubscribeUpdateTransactionInfo) -> anyhow::Result<Value> {
    Ok(json!({
        "signature": Signature::try_from(tx.signature.as_slice()).context("invalid signature")?.to_string(),
        "isVote": tx.is_vote,
        "tx": convert_from::create_tx_with_meta(tx)
            .map_err(|error| anyhow::anyhow!(error))
            .context("invalid tx with meta")?
            .encode(UiTransactionEncoding::Base64, Some(u8::MAX), true)
            .context("failed to encode transaction")?,
    }))
}

fn create_pretty_entry(msg: SubscribeUpdateEntry) -> anyhow::Result<Value> {
    Ok(json!({
        "slot": msg.slot,
        "index": msg.index,
        "numHashes": msg.num_hashes,
        "hash": Hash::new_from_array(<[u8; 32]>::try_from(msg.hash.as_slice()).context("invalid entry hash")?).to_string(),
        "executedTransactionCount": msg.executed_transaction_count,
        "startingTransactionIndex": msg.starting_transaction_index,
    }))
}

fn print_update(kind: &str, filters: &[String], value: Value) {
    info!(
        "{kind} ({}): {}",
        filters.join(","),
        serde_json::to_string(&value).expect("json serialization failed")
    );
}
