use std::{
    borrow::{Borrow, BorrowMut},
    marker::PhantomData,
};

use openmls_traits::storage::{traits, Key, StorageProvider};
use rusqlite::Connection;

use crate::{
    codec::Codec,
    encryption_key_pairs::{
        StorableEncryptionKeyPair, StorableEncryptionKeyPairRef, StorableEncryptionPublicKeyRef,
    },
    epoch_key_pairs::{StorableEpochKeyPairs, StorableEpochKeyPairsRef},
    group_data::{GroupDataType, StorableGroupData, StorableGroupDataRef},
    key_packages::{StorableHashRef, StorableKeyPackage, StorableKeyPackageRef},
    own_leaf_nodes::{StorableLeafNode, StorableLeafNodeRef},
    proposals::{StorableProposal, StorableProposalRef},
    psks::{StorablePskBundle, StorablePskBundleRef, StorablePskIdRef},
    signature_key_pairs::{
        StorableSignatureKeyPairs, StorableSignatureKeyPairsRef, StorableSignaturePublicKeyRef,
    },
    STORAGE_PROVIDER_VERSION,
};

#[cfg(feature = "virtual-clients-draft")]
use crate::vc_secrets::StorableEntityRef as StorableVcSecretRef;

/// The embedded schema migrations, in order. Version numbers and names are the
/// ones the former `refinery` runner recorded, so a database it migrated is
/// recognised as up to date.
const MIGRATIONS: &[(i64, &str, &str)] = &[
    (1, "initial", include_str!("../migrations/V1__initial.sql")),
    (2, "constraint_addition", include_str!("../migrations/V2__constraint_addition.sql")),
    (3, "virtual_clients", include_str!("../migrations/V3__virtual_clients.sql")),
    (4, "vc_emulation_bindings", include_str!("../migrations/V4__vc_emulation_bindings.sql")),
    (5, "vc_operation_secrets", include_str!("../migrations/V5__vc_operation_secrets.sql")),
    (
        6,
        "registered_vc_emulation_epoch",
        include_str!("../migrations/V6__registered_vc_emulation_epoch.sql"),
    ),
];

const MIGRATION_TABLE: &str = "openmls_sqlite_storage_migrations";

fn migration_error(message: &str) -> rusqlite::Error {
    rusqlite::Error::ToSqlConversionFailure(message.into())
}

/// Applies the migrations that are not yet recorded in the migration table,
/// each in its own immediate transaction. The recorded versions must be the
/// leading, correctly named run of [`MIGRATIONS`]: a database from a newer
/// build, or one with a gap or a renamed step, is refused instead of guessed at.
fn migrate(connection: &mut Connection) -> Result<(), rusqlite::Error> {
    connection.execute_batch(&format!(
        "CREATE TABLE IF NOT EXISTS {MIGRATION_TABLE}(version INT4 PRIMARY KEY, name VARCHAR(255), applied_on VARCHAR(255), checksum VARCHAR(255))"
    ))?;
    let recorded: Vec<(i64, String)> = {
        let mut statement = connection
            .prepare(&format!("SELECT version, name FROM {MIGRATION_TABLE} ORDER BY version"))?;
        let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        rows.collect::<Result<_, _>>()?
    };
    if recorded.len() > MIGRATIONS.len() {
        return Err(migration_error("the database is newer than this build"));
    }
    for (known, (version, name)) in MIGRATIONS.iter().zip(&recorded) {
        if known.0 != *version || known.1 != name {
            return Err(migration_error("the recorded migrations do not match this build"));
        }
    }
    for (version, name, sql) in &MIGRATIONS[recorded.len()..] {
        let tx = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        tx.execute_batch(sql)?;
        tx.execute(
            &format!("INSERT INTO {MIGRATION_TABLE}(version, name, applied_on, checksum) VALUES (?1, ?2, datetime('now'), '')"),
            rusqlite::params![version, name],
        )?;
        tx.commit()?;
    }
    Ok(())
}

/// Storage provider for OpenMLS using Sqlite through the `rusqlite` crate.
/// Implements the [`StorageProvider`] trait. The codec used by the storage
/// provider is set by the generic parameter `C`.
pub struct SqliteStorageProvider<C: Codec, ConnectionRef: Borrow<Connection>> {
    connection: ConnectionRef,
    _codec: PhantomData<C>,
}

impl<C: Codec, ConnectionRef: Borrow<Connection>> SqliteStorageProvider<C, ConnectionRef> {
    /// Create a new instance of the [`SqliteStorageProvider`].
    pub fn new(connection: ConnectionRef) -> Self {
        Self {
            connection,
            _codec: PhantomData,
        }
    }
}

impl<C: Codec, ConnectionRef: BorrowMut<Connection>> SqliteStorageProvider<C, ConnectionRef> {
    /// Initialize the database with the necessary tables.
    pub fn run_migrations(&mut self) -> Result<(), rusqlite::Error> {
        migrate(self.connection.borrow_mut())
    }
}

pub(super) struct StorableKeyRef<'a, GroupId: Key<STORAGE_PROVIDER_VERSION>>(pub &'a GroupId);

impl<C: Codec, ConnectionRef: Borrow<Connection>> StorageProvider<STORAGE_PROVIDER_VERSION>
    for SqliteStorageProvider<C, ConnectionRef>
{
    type Error = rusqlite::Error;

    fn write_mls_join_config<
        GroupId: traits::GroupId<STORAGE_PROVIDER_VERSION>,
        MlsGroupJoinConfig: traits::MlsGroupJoinConfig<STORAGE_PROVIDER_VERSION>,
    >(
        &self,
        group_id: &GroupId,
        config: &MlsGroupJoinConfig,
    ) -> Result<(), Self::Error> {
        StorableGroupDataRef(config).store::<C, _>(
            self.connection.borrow(),
            group_id,
            GroupDataType::JoinGroupConfig,
        )
    }

    fn append_own_leaf_node<
        GroupId: traits::GroupId<STORAGE_PROVIDER_VERSION>,
        LeafNode: traits::LeafNode<STORAGE_PROVIDER_VERSION>,
    >(
        &self,
        group_id: &GroupId,
        leaf_node: &LeafNode,
    ) -> Result<(), Self::Error> {
        StorableLeafNodeRef(leaf_node).store::<C, _>(self.connection.borrow(), group_id)
    }

    fn queue_proposal<
        GroupId: traits::GroupId<STORAGE_PROVIDER_VERSION>,
        ProposalRef: traits::ProposalRef<STORAGE_PROVIDER_VERSION>,
        QueuedProposal: traits::QueuedProposal<STORAGE_PROVIDER_VERSION>,
    >(
        &self,
        group_id: &GroupId,
        proposal_ref: &ProposalRef,
        proposal: &QueuedProposal,
    ) -> Result<(), Self::Error> {
        StorableProposalRef(proposal_ref, proposal)
            .store::<C, _>(self.connection.borrow(), group_id)
    }

    fn write_tree<
        GroupId: traits::GroupId<STORAGE_PROVIDER_VERSION>,
        TreeSync: traits::TreeSync<STORAGE_PROVIDER_VERSION>,
    >(
        &self,
        group_id: &GroupId,
        tree: &TreeSync,
    ) -> Result<(), Self::Error> {
        StorableGroupDataRef(tree).store::<C, _>(
            self.connection.borrow(),
            group_id,
            GroupDataType::Tree,
        )
    }

    fn write_interim_transcript_hash<
        GroupId: traits::GroupId<STORAGE_PROVIDER_VERSION>,
        InterimTranscriptHash: traits::InterimTranscriptHash<STORAGE_PROVIDER_VERSION>,
    >(
        &self,
        group_id: &GroupId,
        interim_transcript_hash: &InterimTranscriptHash,
    ) -> Result<(), Self::Error> {
        StorableGroupDataRef(interim_transcript_hash).store::<C, _>(
            self.connection.borrow(),
            group_id,
            GroupDataType::InterimTranscriptHash,
        )
    }

    fn write_context<
        GroupId: traits::GroupId<STORAGE_PROVIDER_VERSION>,
        GroupContext: traits::GroupContext<STORAGE_PROVIDER_VERSION>,
    >(
        &self,
        group_id: &GroupId,
        group_context: &GroupContext,
    ) -> Result<(), Self::Error> {
        StorableGroupDataRef(group_context).store::<C, _>(
            self.connection.borrow(),
            group_id,
            GroupDataType::Context,
        )
    }

    fn write_confirmation_tag<
        GroupId: traits::GroupId<STORAGE_PROVIDER_VERSION>,
        ConfirmationTag: traits::ConfirmationTag<STORAGE_PROVIDER_VERSION>,
    >(
        &self,
        group_id: &GroupId,
        confirmation_tag: &ConfirmationTag,
    ) -> Result<(), Self::Error> {
        StorableGroupDataRef(confirmation_tag).store::<C, _>(
            self.connection.borrow(),
            group_id,
            GroupDataType::ConfirmationTag,
        )
    }

    fn write_group_state<
        GroupState: traits::GroupState<STORAGE_PROVIDER_VERSION>,
        GroupId: traits::GroupId<STORAGE_PROVIDER_VERSION>,
    >(
        &self,
        group_id: &GroupId,
        group_state: &GroupState,
    ) -> Result<(), Self::Error> {
        StorableGroupDataRef(group_state).store::<C, _>(
            self.connection.borrow(),
            group_id,
            GroupDataType::GroupState,
        )
    }

    fn write_message_secrets<
        GroupId: traits::GroupId<STORAGE_PROVIDER_VERSION>,
        MessageSecrets: traits::MessageSecrets<STORAGE_PROVIDER_VERSION>,
    >(
        &self,
        group_id: &GroupId,
        message_secrets: &MessageSecrets,
    ) -> Result<(), Self::Error> {
        StorableGroupDataRef(message_secrets).store::<C, _>(
            self.connection.borrow(),
            group_id,
            GroupDataType::MessageSecrets,
        )?;
        Ok(())
    }

    fn write_resumption_psk_store<
        GroupId: traits::GroupId<STORAGE_PROVIDER_VERSION>,
        ResumptionPskStore: traits::ResumptionPskStore<STORAGE_PROVIDER_VERSION>,
    >(
        &self,
        group_id: &GroupId,
        resumption_psk_store: &ResumptionPskStore,
    ) -> Result<(), Self::Error> {
        StorableGroupDataRef(resumption_psk_store).store::<C, _>(
            self.connection.borrow(),
            group_id,
            GroupDataType::ResumptionPskStore,
        )
    }

    fn write_own_leaf_index<
        GroupId: traits::GroupId<STORAGE_PROVIDER_VERSION>,
        LeafNodeIndex: traits::LeafNodeIndex<STORAGE_PROVIDER_VERSION>,
    >(
        &self,
        group_id: &GroupId,
        own_leaf_index: &LeafNodeIndex,
    ) -> Result<(), Self::Error> {
        StorableGroupDataRef(own_leaf_index).store::<C, _>(
            self.connection.borrow(),
            group_id,
            GroupDataType::OwnLeafIndex,
        )?;
        Ok(())
    }

    fn write_group_epoch_secrets<
        GroupId: traits::GroupId<STORAGE_PROVIDER_VERSION>,
        GroupEpochSecrets: traits::GroupEpochSecrets<STORAGE_PROVIDER_VERSION>,
    >(
        &self,
        group_id: &GroupId,
        group_epoch_secrets: &GroupEpochSecrets,
    ) -> Result<(), Self::Error> {
        StorableGroupDataRef(group_epoch_secrets).store::<C, _>(
            self.connection.borrow(),
            group_id,
            GroupDataType::GroupEpochSecrets,
        )?;
        Ok(())
    }

    fn write_signature_key_pair<
        SignaturePublicKey: traits::SignaturePublicKey<STORAGE_PROVIDER_VERSION>,
        SignatureKeyPair: traits::SignatureKeyPair<STORAGE_PROVIDER_VERSION>,
    >(
        &self,
        public_key: &SignaturePublicKey,
        signature_key_pair: &SignatureKeyPair,
    ) -> Result<(), Self::Error> {
        StorableSignatureKeyPairsRef(signature_key_pair)
            .store::<C, _>(self.connection.borrow(), public_key)
    }

    fn write_encryption_key_pair<
        EncryptionKey: traits::EncryptionKey<STORAGE_PROVIDER_VERSION>,
        HpkeKeyPair: traits::HpkeKeyPair<STORAGE_PROVIDER_VERSION>,
    >(
        &self,
        public_key: &EncryptionKey,
        key_pair: &HpkeKeyPair,
    ) -> Result<(), Self::Error> {
        StorableEncryptionKeyPairRef(key_pair).store::<C, _>(self.connection.borrow(), public_key)
    }

    fn write_encryption_epoch_key_pairs<
        GroupId: traits::GroupId<STORAGE_PROVIDER_VERSION>,
        EpochKey: traits::EpochKey<STORAGE_PROVIDER_VERSION>,
        HpkeKeyPair: traits::HpkeKeyPair<STORAGE_PROVIDER_VERSION>,
    >(
        &self,
        group_id: &GroupId,
        epoch: &EpochKey,
        leaf_index: u32,
        key_pairs: &[HpkeKeyPair],
    ) -> Result<(), Self::Error> {
        StorableEpochKeyPairsRef(key_pairs).store::<C, _, _>(
            self.connection.borrow(),
            group_id,
            epoch,
            leaf_index,
        )
    }

    fn write_key_package<
        HashReference: traits::HashReference<STORAGE_PROVIDER_VERSION>,
        KeyPackage: traits::KeyPackage<STORAGE_PROVIDER_VERSION>,
    >(
        &self,
        hash_ref: &HashReference,
        key_package: &KeyPackage,
    ) -> Result<(), Self::Error> {
        StorableKeyPackageRef(key_package).store::<C, _>(self.connection.borrow(), hash_ref)
    }

    fn write_psk<
        PskId: traits::PskId<STORAGE_PROVIDER_VERSION>,
        PskBundle: traits::PskBundle<STORAGE_PROVIDER_VERSION>,
    >(
        &self,
        psk_id: &PskId,
        psk: &PskBundle,
    ) -> Result<(), Self::Error> {
        StorablePskBundleRef(psk).store::<C, _>(self.connection.borrow(), psk_id)
    }

    fn mls_group_join_config<
        GroupId: traits::GroupId<STORAGE_PROVIDER_VERSION>,
        MlsGroupJoinConfig: traits::MlsGroupJoinConfig<STORAGE_PROVIDER_VERSION>,
    >(
        &self,
        group_id: &GroupId,
    ) -> Result<Option<MlsGroupJoinConfig>, Self::Error> {
        StorableGroupData::load::<C, _>(
            self.connection.borrow(),
            group_id,
            GroupDataType::JoinGroupConfig,
        )
    }

    fn own_leaf_nodes<
        GroupId: traits::GroupId<STORAGE_PROVIDER_VERSION>,
        LeafNode: traits::LeafNode<STORAGE_PROVIDER_VERSION>,
    >(
        &self,
        group_id: &GroupId,
    ) -> Result<Vec<LeafNode>, Self::Error> {
        StorableLeafNode::load::<C, _>(self.connection.borrow(), group_id)
    }

    fn queued_proposal_refs<
        GroupId: traits::GroupId<STORAGE_PROVIDER_VERSION>,
        ProposalRef: traits::ProposalRef<STORAGE_PROVIDER_VERSION>,
    >(
        &self,
        group_id: &GroupId,
    ) -> Result<Vec<ProposalRef>, Self::Error> {
        StorableProposal::<u8, ProposalRef>::load_refs::<C, _>(self.connection.borrow(), group_id)
    }

    fn queued_proposals<
        GroupId: traits::GroupId<STORAGE_PROVIDER_VERSION>,
        ProposalRef: traits::ProposalRef<STORAGE_PROVIDER_VERSION>,
        QueuedProposal: traits::QueuedProposal<STORAGE_PROVIDER_VERSION>,
    >(
        &self,
        group_id: &GroupId,
    ) -> Result<Vec<(ProposalRef, QueuedProposal)>, Self::Error> {
        StorableProposal::load::<C, _>(self.connection.borrow(), group_id)
    }

    fn tree<
        GroupId: traits::GroupId<STORAGE_PROVIDER_VERSION>,
        TreeSync: traits::TreeSync<STORAGE_PROVIDER_VERSION>,
    >(
        &self,
        group_id: &GroupId,
    ) -> Result<Option<TreeSync>, Self::Error> {
        StorableGroupData::load::<C, _>(self.connection.borrow(), group_id, GroupDataType::Tree)
    }

    fn group_context<
        GroupId: traits::GroupId<STORAGE_PROVIDER_VERSION>,
        GroupContext: traits::GroupContext<STORAGE_PROVIDER_VERSION>,
    >(
        &self,
        group_id: &GroupId,
    ) -> Result<Option<GroupContext>, Self::Error> {
        StorableGroupData::load::<C, _>(self.connection.borrow(), group_id, GroupDataType::Context)
    }

    fn interim_transcript_hash<
        GroupId: traits::GroupId<STORAGE_PROVIDER_VERSION>,
        InterimTranscriptHash: traits::InterimTranscriptHash<STORAGE_PROVIDER_VERSION>,
    >(
        &self,
        group_id: &GroupId,
    ) -> Result<Option<InterimTranscriptHash>, Self::Error> {
        StorableGroupData::load::<C, _>(
            self.connection.borrow(),
            group_id,
            GroupDataType::InterimTranscriptHash,
        )
    }

    fn confirmation_tag<
        GroupId: traits::GroupId<STORAGE_PROVIDER_VERSION>,
        ConfirmationTag: traits::ConfirmationTag<STORAGE_PROVIDER_VERSION>,
    >(
        &self,
        group_id: &GroupId,
    ) -> Result<Option<ConfirmationTag>, Self::Error> {
        StorableGroupData::load::<C, _>(
            self.connection.borrow(),
            group_id,
            GroupDataType::ConfirmationTag,
        )
    }

    fn group_state<
        GroupState: traits::GroupState<STORAGE_PROVIDER_VERSION>,
        GroupId: traits::GroupId<STORAGE_PROVIDER_VERSION>,
    >(
        &self,
        group_id: &GroupId,
    ) -> Result<Option<GroupState>, Self::Error> {
        StorableGroupData::load::<C, _>(
            self.connection.borrow(),
            group_id,
            GroupDataType::GroupState,
        )
    }

    fn message_secrets<
        GroupId: traits::GroupId<STORAGE_PROVIDER_VERSION>,
        MessageSecrets: traits::MessageSecrets<STORAGE_PROVIDER_VERSION>,
    >(
        &self,
        group_id: &GroupId,
    ) -> Result<Option<MessageSecrets>, Self::Error> {
        StorableGroupData::load::<C, _>(
            self.connection.borrow(),
            group_id,
            GroupDataType::MessageSecrets,
        )
    }

    fn resumption_psk_store<
        GroupId: traits::GroupId<STORAGE_PROVIDER_VERSION>,
        ResumptionPskStore: traits::ResumptionPskStore<STORAGE_PROVIDER_VERSION>,
    >(
        &self,
        group_id: &GroupId,
    ) -> Result<Option<ResumptionPskStore>, Self::Error> {
        StorableGroupData::load::<C, _>(
            self.connection.borrow(),
            group_id,
            GroupDataType::ResumptionPskStore,
        )
    }

    fn own_leaf_index<
        GroupId: traits::GroupId<STORAGE_PROVIDER_VERSION>,
        LeafNodeIndex: traits::LeafNodeIndex<STORAGE_PROVIDER_VERSION>,
    >(
        &self,
        group_id: &GroupId,
    ) -> Result<Option<LeafNodeIndex>, Self::Error> {
        StorableGroupData::load::<C, _>(
            self.connection.borrow(),
            group_id,
            GroupDataType::OwnLeafIndex,
        )
    }

    fn group_epoch_secrets<
        GroupId: traits::GroupId<STORAGE_PROVIDER_VERSION>,
        GroupEpochSecrets: traits::GroupEpochSecrets<STORAGE_PROVIDER_VERSION>,
    >(
        &self,
        group_id: &GroupId,
    ) -> Result<Option<GroupEpochSecrets>, Self::Error> {
        StorableGroupData::load::<C, _>(
            self.connection.borrow(),
            group_id,
            GroupDataType::GroupEpochSecrets,
        )
    }

    fn signature_key_pair<
        SignaturePublicKey: traits::SignaturePublicKey<STORAGE_PROVIDER_VERSION>,
        SignatureKeyPair: traits::SignatureKeyPair<STORAGE_PROVIDER_VERSION>,
    >(
        &self,
        public_key: &SignaturePublicKey,
    ) -> Result<Option<SignatureKeyPair>, Self::Error> {
        StorableSignatureKeyPairs::load::<C, _>(self.connection.borrow(), public_key)
    }

    fn encryption_key_pair<
        HpkeKeyPair: traits::HpkeKeyPair<STORAGE_PROVIDER_VERSION>,
        EncryptionKey: traits::EncryptionKey<STORAGE_PROVIDER_VERSION>,
    >(
        &self,
        public_key: &EncryptionKey,
    ) -> Result<Option<HpkeKeyPair>, Self::Error> {
        StorableEncryptionKeyPair::load::<C, _>(self.connection.borrow(), public_key)
    }

    fn encryption_epoch_key_pairs<
        GroupId: traits::GroupId<STORAGE_PROVIDER_VERSION>,
        EpochKey: traits::EpochKey<STORAGE_PROVIDER_VERSION>,
        HpkeKeyPair: traits::HpkeKeyPair<STORAGE_PROVIDER_VERSION>,
    >(
        &self,
        group_id: &GroupId,
        epoch: &EpochKey,
        leaf_index: u32,
    ) -> Result<Vec<HpkeKeyPair>, Self::Error> {
        StorableEpochKeyPairs::load::<C, _, _>(
            self.connection.borrow(),
            group_id,
            epoch,
            leaf_index,
        )
    }

    fn key_package<
        KeyPackageRef: traits::HashReference<STORAGE_PROVIDER_VERSION>,
        KeyPackage: traits::KeyPackage<STORAGE_PROVIDER_VERSION>,
    >(
        &self,
        hash_ref: &KeyPackageRef,
    ) -> Result<Option<KeyPackage>, Self::Error> {
        StorableKeyPackage::load::<C, _>(self.connection.borrow(), hash_ref)
    }

    fn psk<
        PskBundle: traits::PskBundle<STORAGE_PROVIDER_VERSION>,
        PskId: traits::PskId<STORAGE_PROVIDER_VERSION>,
    >(
        &self,
        psk_id: &PskId,
    ) -> Result<Option<PskBundle>, Self::Error> {
        StorablePskBundle::load::<C, _>(self.connection.borrow(), psk_id)
    }

    fn remove_proposal<
        GroupId: traits::GroupId<STORAGE_PROVIDER_VERSION>,
        ProposalRef: traits::ProposalRef<STORAGE_PROVIDER_VERSION>,
    >(
        &self,
        group_id: &GroupId,
        proposal_ref: &ProposalRef,
    ) -> Result<(), Self::Error> {
        StorableKeyRef(group_id).delete_proposal::<C, _>(self.connection.borrow(), proposal_ref)
    }

    fn delete_own_leaf_nodes<GroupId: traits::GroupId<STORAGE_PROVIDER_VERSION>>(
        &self,
        group_id: &GroupId,
    ) -> Result<(), Self::Error> {
        StorableKeyRef(group_id).delete_leaf_nodes::<C>(self.connection.borrow())
    }

    fn delete_group_config<GroupId: traits::GroupId<STORAGE_PROVIDER_VERSION>>(
        &self,
        group_id: &GroupId,
    ) -> Result<(), Self::Error> {
        StorableKeyRef(group_id)
            .delete_group_data::<C>(self.connection.borrow(), GroupDataType::JoinGroupConfig)
    }

    fn delete_tree<GroupId: traits::GroupId<STORAGE_PROVIDER_VERSION>>(
        &self,
        group_id: &GroupId,
    ) -> Result<(), Self::Error> {
        StorableKeyRef(group_id)
            .delete_group_data::<C>(self.connection.borrow(), GroupDataType::Tree)
    }

    fn delete_confirmation_tag<GroupId: traits::GroupId<STORAGE_PROVIDER_VERSION>>(
        &self,
        group_id: &GroupId,
    ) -> Result<(), Self::Error> {
        StorableKeyRef(group_id)
            .delete_group_data::<C>(self.connection.borrow(), GroupDataType::ConfirmationTag)
    }

    fn delete_group_state<GroupId: traits::GroupId<STORAGE_PROVIDER_VERSION>>(
        &self,
        group_id: &GroupId,
    ) -> Result<(), Self::Error> {
        StorableKeyRef(group_id)
            .delete_group_data::<C>(self.connection.borrow(), GroupDataType::GroupState)
    }

    fn delete_context<GroupId: traits::GroupId<STORAGE_PROVIDER_VERSION>>(
        &self,
        group_id: &GroupId,
    ) -> Result<(), Self::Error> {
        StorableKeyRef(group_id)
            .delete_group_data::<C>(self.connection.borrow(), GroupDataType::Context)
    }

    fn delete_interim_transcript_hash<GroupId: traits::GroupId<STORAGE_PROVIDER_VERSION>>(
        &self,
        group_id: &GroupId,
    ) -> Result<(), Self::Error> {
        StorableKeyRef(group_id).delete_group_data::<C>(
            self.connection.borrow(),
            GroupDataType::InterimTranscriptHash,
        )
    }

    fn delete_message_secrets<GroupId: traits::GroupId<STORAGE_PROVIDER_VERSION>>(
        &self,
        group_id: &GroupId,
    ) -> Result<(), Self::Error> {
        StorableKeyRef(group_id)
            .delete_group_data::<C>(self.connection.borrow(), GroupDataType::MessageSecrets)
    }

    fn delete_all_resumption_psk_secrets<GroupId: traits::GroupId<STORAGE_PROVIDER_VERSION>>(
        &self,
        group_id: &GroupId,
    ) -> Result<(), Self::Error> {
        StorableKeyRef(group_id)
            .delete_group_data::<C>(self.connection.borrow(), GroupDataType::ResumptionPskStore)
    }

    fn delete_own_leaf_index<GroupId: traits::GroupId<STORAGE_PROVIDER_VERSION>>(
        &self,
        group_id: &GroupId,
    ) -> Result<(), Self::Error> {
        StorableKeyRef(group_id)
            .delete_group_data::<C>(self.connection.borrow(), GroupDataType::OwnLeafIndex)
    }

    fn delete_group_epoch_secrets<GroupId: traits::GroupId<STORAGE_PROVIDER_VERSION>>(
        &self,
        group_id: &GroupId,
    ) -> Result<(), Self::Error> {
        StorableKeyRef(group_id)
            .delete_group_data::<C>(self.connection.borrow(), GroupDataType::GroupEpochSecrets)
    }

    fn clear_proposal_queue<
        GroupId: traits::GroupId<STORAGE_PROVIDER_VERSION>,
        ProposalRef: traits::ProposalRef<STORAGE_PROVIDER_VERSION>,
    >(
        &self,
        group_id: &GroupId,
    ) -> Result<(), Self::Error> {
        StorableKeyRef(group_id).delete_all_proposals::<C>(self.connection.borrow())?;
        Ok(())
    }

    fn delete_signature_key_pair<
        SignaturePublicKey: traits::SignaturePublicKey<STORAGE_PROVIDER_VERSION>,
    >(
        &self,
        public_key: &SignaturePublicKey,
    ) -> Result<(), Self::Error> {
        StorableSignaturePublicKeyRef(public_key).delete::<C>(self.connection.borrow())
    }

    fn delete_encryption_key_pair<
        EncryptionKey: traits::EncryptionKey<STORAGE_PROVIDER_VERSION>,
    >(
        &self,
        public_key: &EncryptionKey,
    ) -> Result<(), Self::Error> {
        StorableEncryptionPublicKeyRef(public_key).delete::<C>(self.connection.borrow())
    }

    fn delete_encryption_epoch_key_pairs<
        GroupId: traits::GroupId<STORAGE_PROVIDER_VERSION>,
        EpochKey: traits::EpochKey<STORAGE_PROVIDER_VERSION>,
    >(
        &self,
        group_id: &GroupId,
        epoch: &EpochKey,
        leaf_index: u32,
    ) -> Result<(), Self::Error> {
        StorableKeyRef(group_id).delete_epoch_key_pair::<C, _>(
            self.connection.borrow(),
            epoch,
            leaf_index,
        )
    }

    fn delete_key_package<KeyPackageRef: traits::HashReference<STORAGE_PROVIDER_VERSION>>(
        &self,
        hash_ref: &KeyPackageRef,
    ) -> Result<(), Self::Error> {
        #[cfg(feature = "virtual-clients-draft")]
        StorableKeyRef(hash_ref)
            .delete_retained_key_package_material::<C>(self.connection.borrow())?;
        StorableHashRef(hash_ref).delete_key_package::<C>(self.connection.borrow())
    }

    fn delete_psk<PskKey: traits::PskId<STORAGE_PROVIDER_VERSION>>(
        &self,
        psk_id: &PskKey,
    ) -> Result<(), Self::Error> {
        StorablePskIdRef(psk_id).delete::<C>(self.connection.borrow())
    }

    #[cfg(feature = "extensions-draft")]
    fn write_application_export_tree<
        GroupId: traits::GroupId<STORAGE_PROVIDER_VERSION>,
        ApplicationExportTree: traits::ApplicationExportTree<STORAGE_PROVIDER_VERSION>,
    >(
        &self,
        group_id: &GroupId,
        application_export_tree: &ApplicationExportTree,
    ) -> Result<(), Self::Error> {
        StorableGroupDataRef(application_export_tree).store::<C, _>(
            self.connection.borrow(),
            group_id,
            GroupDataType::ApplicationExportTree,
        )
    }

    #[cfg(feature = "extensions-draft")]
    fn application_export_tree<
        GroupId: traits::GroupId<STORAGE_PROVIDER_VERSION>,
        ApplicationExportTree: traits::ApplicationExportTree<STORAGE_PROVIDER_VERSION>,
    >(
        &self,
        group_id: &GroupId,
    ) -> Result<Option<ApplicationExportTree>, Self::Error> {
        StorableGroupData::load::<C, _>(
            self.connection.borrow(),
            group_id,
            GroupDataType::ApplicationExportTree,
        )
    }

    #[cfg(feature = "extensions-draft")]
    fn delete_application_export_tree<
        GroupId: traits::GroupId<STORAGE_PROVIDER_VERSION>,
        ApplicationExportTree: traits::ApplicationExportTree<STORAGE_PROVIDER_VERSION>,
    >(
        &self,
        group_id: &GroupId,
    ) -> Result<(), Self::Error> {
        StorableKeyRef(group_id).delete_group_data::<C>(
            self.connection.borrow(),
            GroupDataType::ApplicationExportTree,
        )
    }

    #[cfg(feature = "virtual-clients-draft")]
    fn write_vc_emulation_epoch_state<
        EpochId: traits::VcEpochId<STORAGE_PROVIDER_VERSION>,
        VcEmulationEpochState: traits::VcEmulationEpochState<STORAGE_PROVIDER_VERSION>,
    >(
        &self,
        epoch_id: &EpochId,
        vc_emulation_epoch_state: &VcEmulationEpochState,
    ) -> Result<(), Self::Error> {
        StorableVcSecretRef(vc_emulation_epoch_state)
            .store_vc_emulation_epoch_state::<C, _>(self.connection.borrow(), epoch_id)
    }

    #[cfg(feature = "virtual-clients-draft")]
    fn vc_emulation_epoch_state<
        EpochId: traits::VcEpochId<STORAGE_PROVIDER_VERSION>,
        VcEmulationEpochState: traits::VcEmulationEpochState<STORAGE_PROVIDER_VERSION>,
    >(
        &self,
        epoch_id: &EpochId,
    ) -> Result<Option<VcEmulationEpochState>, Self::Error> {
        StorableKeyRef(epoch_id)
            .load_vc_emulation_epoch_state::<C, VcEmulationEpochState>(self.connection.borrow())
    }

    #[cfg(feature = "virtual-clients-draft")]
    fn delete_vc_emulation_state_if_unreferenced<
        EpochId: traits::VcEpochId<STORAGE_PROVIDER_VERSION>,
    >(
        &self,
        epoch_id: &EpochId,
    ) -> Result<bool, Self::Error> {
        // The application should call this within a transaction so the liveness
        // check and the deletions apply atomically and a material stored
        // concurrently cannot be orphaned.
        if StorableKeyRef(epoch_id)
            .has_retained_key_package_material_for_epoch::<C>(self.connection.borrow())?
        {
            return Ok(false);
        }
        StorableKeyRef(epoch_id).delete_vc_emulation_epoch_state::<C>(self.connection.borrow())?;
        StorableKeyRef(epoch_id).delete_vc_operation_tree::<C>(self.connection.borrow())?;
        Ok(true)
    }

    #[cfg(feature = "virtual-clients-draft")]
    fn write_vc_emulation_bindings<
        GroupId: traits::GroupId<STORAGE_PROVIDER_VERSION>,
        VcEmulationBindings: traits::VcEmulationBindings<STORAGE_PROVIDER_VERSION>,
    >(
        &self,
        group_id: &GroupId,
        bindings: &VcEmulationBindings,
    ) -> Result<(), Self::Error> {
        crate::vc_secrets::StorableEmulationBindingRef(bindings)
            .store_vc_emulation_bindings::<C, _>(self.connection.borrow(), group_id)
    }

    #[cfg(feature = "virtual-clients-draft")]
    fn vc_emulation_bindings<
        GroupId: traits::GroupId<STORAGE_PROVIDER_VERSION>,
        VcEmulationBindings: traits::VcEmulationBindings<STORAGE_PROVIDER_VERSION>,
    >(
        &self,
        group_id: &GroupId,
    ) -> Result<Option<VcEmulationBindings>, Self::Error> {
        StorableKeyRef(group_id)
            .load_vc_emulation_bindings::<C, VcEmulationBindings>(self.connection.borrow())
    }

    #[cfg(feature = "virtual-clients-draft")]
    fn delete_vc_emulation_bindings<GroupId: traits::GroupId<STORAGE_PROVIDER_VERSION>>(
        &self,
        group_id: &GroupId,
    ) -> Result<(), Self::Error> {
        StorableKeyRef(group_id).delete_vc_emulation_bindings::<C>(self.connection.borrow())
    }

    #[cfg(feature = "virtual-clients-draft")]
    fn write_registered_vc_emulation_epoch<
        GroupId: traits::GroupId<STORAGE_PROVIDER_VERSION>,
        RegisteredVcEmulationEpoch: traits::RegisteredVcEmulationEpoch<STORAGE_PROVIDER_VERSION>,
    >(
        &self,
        group_id: &GroupId,
        registered: &RegisteredVcEmulationEpoch,
    ) -> Result<(), Self::Error> {
        crate::vc_secrets::StorableRegisteredVcEmulationEpochRef(registered)
            .store_registered_vc_emulation_epoch::<C, _>(self.connection.borrow(), group_id)
    }

    #[cfg(feature = "virtual-clients-draft")]
    fn registered_vc_emulation_epoch<
        GroupId: traits::GroupId<STORAGE_PROVIDER_VERSION>,
        RegisteredVcEmulationEpoch: traits::RegisteredVcEmulationEpoch<STORAGE_PROVIDER_VERSION>,
    >(
        &self,
        group_id: &GroupId,
    ) -> Result<Option<RegisteredVcEmulationEpoch>, Self::Error> {
        StorableKeyRef(group_id)
            .load_registered_vc_emulation_epoch::<C, RegisteredVcEmulationEpoch>(
                self.connection.borrow(),
            )
    }

    #[cfg(feature = "virtual-clients-draft")]
    fn delete_registered_vc_emulation_epoch<GroupId: traits::GroupId<STORAGE_PROVIDER_VERSION>>(
        &self,
        group_id: &GroupId,
    ) -> Result<(), Self::Error> {
        StorableKeyRef(group_id).delete_registered_vc_emulation_epoch::<C>(self.connection.borrow())
    }

    #[cfg(feature = "virtual-clients-draft")]
    fn write_vc_operation_tree<
        EpochId: traits::VcEpochId<STORAGE_PROVIDER_VERSION>,
        VcOperationTree: traits::VcOperationTree<STORAGE_PROVIDER_VERSION>,
    >(
        &self,
        epoch_id: &EpochId,
        vc_operation_tree: &VcOperationTree,
    ) -> Result<(), Self::Error> {
        crate::vc_secrets::StorableOperationTreeRef(vc_operation_tree)
            .store_vc_operation_tree::<C, _>(self.connection.borrow(), epoch_id)
    }

    #[cfg(feature = "virtual-clients-draft")]
    fn vc_operation_tree<
        EpochId: traits::VcEpochId<STORAGE_PROVIDER_VERSION>,
        VcOperationTree: traits::VcOperationTree<STORAGE_PROVIDER_VERSION>,
    >(
        &self,
        epoch_id: &EpochId,
    ) -> Result<Option<VcOperationTree>, Self::Error> {
        StorableKeyRef(epoch_id)
            .load_vc_operation_tree::<C, VcOperationTree>(self.connection.borrow())
    }

    #[cfg(feature = "virtual-clients-draft")]
    fn write_retained_key_package_material_batch<
        EpochId: traits::VcEpochId<STORAGE_PROVIDER_VERSION>,
        VcOperationTree: traits::VcOperationTree<STORAGE_PROVIDER_VERSION>,
        KeyPackageRef: traits::HashReference<STORAGE_PROVIDER_VERSION>,
        RetainedKeyPackageMaterial: traits::RetainedKeyPackageMaterial<STORAGE_PROVIDER_VERSION>,
    >(
        &self,
        epoch_id: &EpochId,
        operation_tree: &VcOperationTree,
        materials: &[(KeyPackageRef, RetainedKeyPackageMaterial)],
    ) -> Result<(), Self::Error> {
        // The application should call this within a transaction so the advanced
        // tree and all materials are written together and roll back together on
        // error, leaving the tree unadvanced.
        crate::vc_secrets::StorableOperationTreeRef(operation_tree)
            .store_vc_operation_tree::<C, _>(self.connection.borrow(), epoch_id)?;
        for (hash_ref, record) in materials {
            crate::vc_secrets::StorableRetainedKeyPackageMaterialRef(record)
                .store_retained_key_package_material::<C, _, _>(
                    self.connection.borrow(),
                    epoch_id,
                    hash_ref,
                )?;
        }
        Ok(())
    }

    #[cfg(feature = "virtual-clients-draft")]
    fn retained_key_package_material<
        KeyPackageRef: traits::HashReference<STORAGE_PROVIDER_VERSION>,
        RetainedKeyPackageMaterial: traits::RetainedKeyPackageMaterial<STORAGE_PROVIDER_VERSION>,
    >(
        &self,
        hash_ref: &KeyPackageRef,
    ) -> Result<Option<RetainedKeyPackageMaterial>, Self::Error> {
        StorableKeyRef(hash_ref)
            .load_retained_key_package_material::<C, RetainedKeyPackageMaterial>(
                self.connection.borrow(),
            )
    }

    #[cfg(feature = "virtual-clients-draft")]
    fn has_retained_key_package_material_for_epoch<
        EpochId: traits::VcEpochId<STORAGE_PROVIDER_VERSION>,
    >(
        &self,
        epoch_id: &EpochId,
    ) -> Result<bool, Self::Error> {
        StorableKeyRef(epoch_id)
            .has_retained_key_package_material_for_epoch::<C>(self.connection.borrow())
    }

    #[cfg(feature = "virtual-clients-draft")]
    fn delete_retained_key_package_material<
        KeyPackageRef: traits::HashReference<STORAGE_PROVIDER_VERSION>,
    >(
        &self,
        hash_ref: &KeyPackageRef,
    ) -> Result<(), Self::Error> {
        StorableKeyRef(hash_ref).delete_retained_key_package_material::<C>(self.connection.borrow())
    }
}

#[cfg(test)]
mod migration_tests {
    use super::{migrate, MIGRATIONS, MIGRATION_TABLE};
    use rusqlite::Connection;

    fn recorded(connection: &Connection) -> i64 {
        connection
            .query_row(&format!("SELECT count(*) FROM {MIGRATION_TABLE}"), [], |row| row.get(0))
            .unwrap()
    }

    #[test]
    fn applies_every_migration_once() {
        let mut connection = Connection::open_in_memory().unwrap();
        migrate(&mut connection).unwrap();
        migrate(&mut connection).unwrap();
        assert_eq!(recorded(&connection), MIGRATIONS.len() as i64);
        connection
            .execute_batch("SELECT 1 FROM openmls_group_data LIMIT 1")
            .unwrap();
    }

    #[test]
    fn recognises_a_database_the_previous_runner_migrated() {
        let mut connection = Connection::open_in_memory().unwrap();
        migrate(&mut connection).unwrap();
        connection
            .execute_batch(&format!("UPDATE {MIGRATION_TABLE} SET checksum = '123456789', applied_on = '2026-01-01T00:00:00Z'"))
            .unwrap();
        migrate(&mut connection).unwrap();
        assert_eq!(recorded(&connection), MIGRATIONS.len() as i64);
    }

    #[test]
    fn refuses_a_newer_or_inconsistent_database() {
        let mut newer = Connection::open_in_memory().unwrap();
        migrate(&mut newer).unwrap();
        newer
            .execute_batch(&format!("INSERT INTO {MIGRATION_TABLE}(version, name) VALUES (99, 'future')"))
            .unwrap();
        assert!(migrate(&mut newer).is_err());

        let mut renamed = Connection::open_in_memory().unwrap();
        migrate(&mut renamed).unwrap();
        renamed
            .execute_batch(&format!("UPDATE {MIGRATION_TABLE} SET name = 'other' WHERE version = 2"))
            .unwrap();
        assert!(migrate(&mut renamed).is_err());

        let mut gap = Connection::open_in_memory().unwrap();
        migrate(&mut gap).unwrap();
        gap.execute_batch(&format!("DELETE FROM {MIGRATION_TABLE} WHERE version = 1"))
            .unwrap();
        assert!(migrate(&mut gap).is_err());
    }
}
