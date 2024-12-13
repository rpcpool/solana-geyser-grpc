use {
    crate::{
        convert_to,
        geyser::{
            subscribe_update::UpdateOneof, CommitmentLevel as CommitmentLevelProto,
            SubscribeUpdateAccount, SubscribeUpdateAccountInfo, SubscribeUpdateBlock,
            SubscribeUpdateBlockMeta, SubscribeUpdateEntry, SubscribeUpdateSlot,
            SubscribeUpdateTransaction, SubscribeUpdateTransactionInfo,
        },
        solana::storage::confirmed_block,
    },
    agave_geyser_plugin_interface::geyser_plugin_interface::{
        ReplicaAccountInfoV3, ReplicaBlockInfoV4, ReplicaEntryInfoV2, ReplicaTransactionInfoV2,
        SlotStatus,
    },
    prost_types::Timestamp,
    solana_sdk::{
        clock::Slot,
        hash::{Hash, HASH_BYTES},
        pubkey::Pubkey,
        signature::Signature,
    },
    std::{
        collections::HashSet,
        ops::{Deref, DerefMut},
        sync::Arc,
        time::SystemTime,
    },
};

type FromUpdateOneofResult<T> = Result<T, &'static str>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CommitmentLevel {
    Processed,
    Confirmed,
    Finalized,
    FirstShredReceived,
    Completed,
    CreatedBank,
    Dead,
}

impl From<&SlotStatus> for CommitmentLevel {
    fn from(status: &SlotStatus) -> Self {
        match status {
            SlotStatus::Processed => Self::Processed,
            SlotStatus::Confirmed => Self::Confirmed,
            SlotStatus::Rooted => Self::Finalized,
            SlotStatus::FirstShredReceived => Self::FirstShredReceived,
            SlotStatus::Completed => Self::Completed,
            SlotStatus::CreatedBank => Self::CreatedBank,
            SlotStatus::Dead(_error) => Self::Dead,
        }
    }
}

impl From<CommitmentLevel> for CommitmentLevelProto {
    fn from(commitment: CommitmentLevel) -> Self {
        match commitment {
            CommitmentLevel::Processed => Self::Processed,
            CommitmentLevel::Confirmed => Self::Confirmed,
            CommitmentLevel::Finalized => Self::Finalized,
            CommitmentLevel::FirstShredReceived => Self::FirstShredReceived,
            CommitmentLevel::Completed => Self::Completed,
            CommitmentLevel::CreatedBank => Self::CreatedBank,
            CommitmentLevel::Dead => Self::Dead,
        }
    }
}

impl From<CommitmentLevelProto> for CommitmentLevel {
    fn from(status: CommitmentLevelProto) -> Self {
        match status {
            CommitmentLevelProto::Processed => Self::Processed,
            CommitmentLevelProto::Confirmed => Self::Confirmed,
            CommitmentLevelProto::Finalized => Self::Finalized,
            CommitmentLevelProto::FirstShredReceived => Self::FirstShredReceived,
            CommitmentLevelProto::Completed => Self::Completed,
            CommitmentLevelProto::CreatedBank => Self::CreatedBank,
            CommitmentLevelProto::Dead => Self::Dead,
        }
    }
}

impl CommitmentLevel {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Processed => "processed",
            Self::Confirmed => "confirmed",
            Self::Finalized => "finalized",
            Self::FirstShredReceived => "first_shread_received",
            Self::Completed => "completed",
            Self::CreatedBank => "created_bank",
            Self::Dead => "dead",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MessageSlot {
    pub slot: Slot,
    pub parent: Option<Slot>,
    pub status: CommitmentLevel,
    pub dead_error: Option<String>,
    pub created_at: Timestamp,
}

impl MessageSlot {
    pub fn from_geyser(slot: Slot, parent: Option<Slot>, status: &SlotStatus) -> Self {
        Self {
            slot,
            parent,
            status: status.into(),
            dead_error: if let SlotStatus::Dead(error) = status {
                Some(error.clone())
            } else {
                None
            },
            created_at: Timestamp::from(SystemTime::now()),
        }
    }

    pub fn from_update_oneof(
        msg: &SubscribeUpdateSlot,
        created_at: Timestamp,
    ) -> FromUpdateOneofResult<Self> {
        Ok(Self {
            slot: msg.slot,
            parent: msg.parent,
            status: CommitmentLevelProto::try_from(msg.status)
                .map_err(|_| "failed to parse commitment level")?
                .into(),
            dead_error: msg.dead_error.clone(),
            created_at,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MessageAccountInfo {
    pub pubkey: Pubkey,
    pub lamports: u64,
    pub owner: Pubkey,
    pub executable: bool,
    pub rent_epoch: u64,
    pub data: Vec<u8>,
    pub write_version: u64,
    pub txn_signature: Option<Signature>,
}

impl MessageAccountInfo {
    pub fn from_geyser(info: &ReplicaAccountInfoV3<'_>) -> Self {
        Self {
            pubkey: Pubkey::try_from(info.pubkey).expect("valid Pubkey"),
            lamports: info.lamports,
            owner: Pubkey::try_from(info.owner).expect("valid Pubkey"),
            executable: info.executable,
            rent_epoch: info.rent_epoch,
            data: info.data.into(),
            write_version: info.write_version,
            txn_signature: info.txn.map(|txn| *txn.signature()),
        }
    }

    pub fn from_update_oneof(msg: SubscribeUpdateAccountInfo) -> FromUpdateOneofResult<Self> {
        Ok(Self {
            pubkey: Pubkey::try_from(msg.pubkey.as_slice()).map_err(|_| "invalid pubkey length")?,
            lamports: msg.lamports,
            owner: Pubkey::try_from(msg.owner.as_slice()).map_err(|_| "invalid owner length")?,
            executable: msg.executable,
            rent_epoch: msg.rent_epoch,
            data: msg.data,
            write_version: msg.write_version,
            txn_signature: msg
                .txn_signature
                .map(|sig| {
                    Signature::try_from(sig.as_slice()).map_err(|_| "invalid signature length")
                })
                .transpose()?,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MessageAccount {
    pub account: Arc<MessageAccountInfo>,
    pub slot: Slot,
    pub is_startup: bool,
    pub created_at: Timestamp,
}

impl MessageAccount {
    pub fn from_geyser(info: &ReplicaAccountInfoV3<'_>, slot: Slot, is_startup: bool) -> Self {
        Self {
            account: Arc::new(MessageAccountInfo::from_geyser(info)),
            slot,
            is_startup,
            created_at: Timestamp::from(SystemTime::now()),
        }
    }

    pub fn from_update_oneof(
        msg: SubscribeUpdateAccount,
        created_at: Timestamp,
    ) -> FromUpdateOneofResult<Self> {
        Ok(Self {
            account: Arc::new(MessageAccountInfo::from_update_oneof(
                msg.account.ok_or("account message should be defined")?,
            )?),
            slot: msg.slot,
            is_startup: msg.is_startup,
            created_at,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MessageTransactionInfo {
    pub signature: Signature,
    pub is_vote: bool,
    pub transaction: confirmed_block::Transaction,
    pub meta: confirmed_block::TransactionStatusMeta,
    pub index: usize,
    pub account_keys: HashSet<Pubkey>,
}

impl MessageTransactionInfo {
    pub fn from_geyser(info: &ReplicaTransactionInfoV2<'_>) -> Self {
        let account_keys = info
            .transaction
            .message()
            .account_keys()
            .iter()
            .copied()
            .collect();

        Self {
            signature: *info.signature,
            is_vote: info.is_vote,
            transaction: convert_to::create_transaction(info.transaction),
            meta: convert_to::create_transaction_meta(info.transaction_status_meta),
            index: info.index,
            account_keys,
        }
    }

    pub fn from_update_oneof(msg: SubscribeUpdateTransactionInfo) -> FromUpdateOneofResult<Self> {
        Ok(Self {
            signature: Signature::try_from(msg.signature.as_slice())
                .map_err(|_| "invalid signature length")?,
            is_vote: msg.is_vote,
            transaction: msg
                .transaction
                .ok_or("transaction message should be defined")?,
            meta: msg.meta.ok_or("meta message should be defined")?,
            index: msg.index as usize,
            account_keys: HashSet::new(),
        })
    }

    pub fn fill_account_keys(&mut self) -> FromUpdateOneofResult<()> {
        let mut account_keys = HashSet::new();

        // static
        if let Some(pubkeys) = self
            .transaction
            .message
            .as_ref()
            .map(|msg| msg.account_keys.as_slice())
        {
            for pubkey in pubkeys {
                account_keys.insert(
                    Pubkey::try_from(pubkey.as_slice()).map_err(|_| "invalid pubkey length")?,
                );
            }
        }

        // dynamic
        for pubkey in self.meta.loaded_writable_addresses.iter() {
            account_keys
                .insert(Pubkey::try_from(pubkey.as_slice()).map_err(|_| "invalid pubkey length")?);
        }
        for pubkey in self.meta.loaded_readonly_addresses.iter() {
            account_keys
                .insert(Pubkey::try_from(pubkey.as_slice()).map_err(|_| "invalid pubkey length")?);
        }

        self.account_keys = account_keys;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MessageTransaction {
    pub transaction: Arc<MessageTransactionInfo>,
    pub slot: u64,
    pub created_at: Timestamp,
}

impl MessageTransaction {
    pub fn from_geyser(info: &ReplicaTransactionInfoV2<'_>, slot: Slot) -> Self {
        Self {
            transaction: Arc::new(MessageTransactionInfo::from_geyser(info)),
            slot,
            created_at: Timestamp::from(SystemTime::now()),
        }
    }

    pub fn from_update_oneof(
        msg: SubscribeUpdateTransaction,
        created_at: Timestamp,
    ) -> FromUpdateOneofResult<Self> {
        Ok(Self {
            transaction: Arc::new(MessageTransactionInfo::from_update_oneof(
                msg.transaction
                    .ok_or("transaction message should be defined")?,
            )?),
            slot: msg.slot,
            created_at,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MessageEntry {
    pub slot: u64,
    pub index: usize,
    pub num_hashes: u64,
    pub hash: Hash,
    pub executed_transaction_count: u64,
    pub starting_transaction_index: u64,
    pub created_at: Timestamp,
}

impl MessageEntry {
    pub fn from_geyser(info: &ReplicaEntryInfoV2) -> Self {
        Self {
            slot: info.slot,
            index: info.index,
            num_hashes: info.num_hashes,
            hash: Hash::new(info.hash),
            executed_transaction_count: info.executed_transaction_count,
            starting_transaction_index: info
                .starting_transaction_index
                .try_into()
                .expect("failed convert usize to u64"),
            created_at: Timestamp::from(SystemTime::now()),
        }
    }

    pub fn from_update_oneof(
        msg: &SubscribeUpdateEntry,
        created_at: Timestamp,
    ) -> FromUpdateOneofResult<Self> {
        Ok(Self {
            slot: msg.slot,
            index: msg.index as usize,
            num_hashes: msg.num_hashes,
            hash: Hash::new_from_array(
                <[u8; HASH_BYTES]>::try_from(msg.hash.as_slice())
                    .map_err(|_| "invalid hash length")?,
            ),
            executed_transaction_count: msg.executed_transaction_count,
            starting_transaction_index: msg.starting_transaction_index,
            created_at,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MessageBlockMeta {
    pub block_meta: SubscribeUpdateBlockMeta,
    pub created_at: Timestamp,
}

impl Deref for MessageBlockMeta {
    type Target = SubscribeUpdateBlockMeta;

    fn deref(&self) -> &Self::Target {
        &self.block_meta
    }
}

impl DerefMut for MessageBlockMeta {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.block_meta
    }
}

impl MessageBlockMeta {
    pub fn from_geyser(info: &ReplicaBlockInfoV4<'_>) -> Self {
        Self {
            block_meta: SubscribeUpdateBlockMeta {
                parent_slot: info.parent_slot,
                slot: info.slot,
                parent_blockhash: info.parent_blockhash.to_string(),
                blockhash: info.blockhash.to_string(),
                rewards: Some(convert_to::create_rewards_obj(
                    &info.rewards.rewards,
                    info.rewards.num_partitions,
                )),
                block_time: info.block_time.map(convert_to::create_timestamp),
                block_height: info.block_height.map(convert_to::create_block_height),
                executed_transaction_count: info.executed_transaction_count,
                entries_count: info.entry_count,
            },
            created_at: Timestamp::from(SystemTime::now()),
        }
    }

    pub const fn from_update_oneof(
        block_meta: SubscribeUpdateBlockMeta,
        created_at: Timestamp,
    ) -> Self {
        Self {
            block_meta,
            created_at,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MessageBlock {
    pub meta: Arc<MessageBlockMeta>,
    pub transactions: Vec<Arc<MessageTransactionInfo>>,
    pub updated_account_count: u64,
    pub accounts: Vec<Arc<MessageAccountInfo>>,
    pub entries: Vec<Arc<MessageEntry>>,
    pub created_at: Timestamp,
}

impl MessageBlock {
    pub fn new(
        meta: Arc<MessageBlockMeta>,
        transactions: Vec<Arc<MessageTransactionInfo>>,
        accounts: Vec<Arc<MessageAccountInfo>>,
        entries: Vec<Arc<MessageEntry>>,
    ) -> Self {
        Self {
            meta,
            transactions,
            updated_account_count: accounts.len() as u64,
            accounts,
            entries,
            created_at: Timestamp::from(SystemTime::now()),
        }
    }

    pub fn from_update_oneof(
        msg: SubscribeUpdateBlock,
        created_at: Timestamp,
    ) -> FromUpdateOneofResult<Self> {
        Ok(Self {
            meta: Arc::new(MessageBlockMeta {
                block_meta: SubscribeUpdateBlockMeta {
                    slot: msg.slot,
                    blockhash: msg.blockhash,
                    rewards: msg.rewards,
                    block_time: msg.block_time,
                    block_height: msg.block_height,
                    parent_slot: msg.parent_slot,
                    parent_blockhash: msg.parent_blockhash,
                    executed_transaction_count: msg.executed_transaction_count,
                    entries_count: msg.entries_count,
                },
                created_at,
            }),
            transactions: msg
                .transactions
                .into_iter()
                .map(|tx| MessageTransactionInfo::from_update_oneof(tx).map(Arc::new))
                .collect::<Result<Vec<_>, _>>()?,
            updated_account_count: msg.updated_account_count,
            accounts: msg
                .accounts
                .into_iter()
                .map(|account| MessageAccountInfo::from_update_oneof(account).map(Arc::new))
                .collect::<Result<Vec<_>, _>>()?,
            entries: msg
                .entries
                .iter()
                .map(|entry| MessageEntry::from_update_oneof(entry, created_at).map(Arc::new))
                .collect::<Result<Vec<_>, _>>()?,
            created_at,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Message {
    Slot(MessageSlot),
    Account(MessageAccount),
    Transaction(MessageTransaction),
    Entry(Arc<MessageEntry>),
    BlockMeta(Arc<MessageBlockMeta>),
    Block(Arc<MessageBlock>),
}

impl Message {
    pub fn get_slot(&self) -> u64 {
        match self {
            Self::Slot(msg) => msg.slot,
            Self::Account(msg) => msg.slot,
            Self::Transaction(msg) => msg.slot,
            Self::Entry(msg) => msg.slot,
            Self::BlockMeta(msg) => msg.slot,
            Self::Block(msg) => msg.meta.slot,
        }
    }

    pub fn from_update_oneof(
        oneof: UpdateOneof,
        created_at: Timestamp,
    ) -> FromUpdateOneofResult<Self> {
        Ok(match oneof {
            UpdateOneof::Account(msg) => {
                Self::Account(MessageAccount::from_update_oneof(msg, created_at)?)
            }
            UpdateOneof::Slot(msg) => Self::Slot(MessageSlot::from_update_oneof(&msg, created_at)?),
            UpdateOneof::Transaction(msg) => {
                Self::Transaction(MessageTransaction::from_update_oneof(msg, created_at)?)
            }
            UpdateOneof::TransactionStatus(_) => {
                return Err("TransactionStatus message is not supported")
            }
            UpdateOneof::Block(msg) => {
                Self::Block(Arc::new(MessageBlock::from_update_oneof(msg, created_at)?))
            }
            UpdateOneof::Ping(_) => return Err("Ping message is not supported"),
            UpdateOneof::Pong(_) => return Err("Pong message is not supported"),
            UpdateOneof::BlockMeta(msg) => Self::BlockMeta(Arc::new(
                MessageBlockMeta::from_update_oneof(msg, created_at),
            )),
            UpdateOneof::Entry(msg) => {
                Self::Entry(Arc::new(MessageEntry::from_update_oneof(&msg, created_at)?))
            }
        })
    }
}
