//! The OpenMLS-backed engine.
//!
//! One [`MlsEngine`] represents one **device**. It owns an `OpenMlsRustCrypto`
//! provider (which stores all key + group state) and the device's signing
//! identity. All public methods take and return plain bytes so callers never
//! depend on OpenMLS types directly. See the crate docs for the operation map.

use openmls::prelude::tls_codec::{Deserialize as _, Serialize as _};
use openmls::prelude::{
    BasicCredential, Ciphersuite, CredentialWithKey, GroupId, KeyPackage, KeyPackageIn, MlsGroup,
    MlsGroupCreateConfig, MlsGroupJoinConfig, MlsMessageBodyIn, MlsMessageIn,
    ProcessedMessageContent, ProtocolVersion, StagedWelcome,
};
use openmls_basic_credential::SignatureKeyPair;
use openmls_rust_crypto::OpenMlsRustCrypto;
use openmls_traits::OpenMlsProvider;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use accord_crypto::identity::IdentityKeyPair;

use crate::MlsError;

/// On-disk / backup form of an engine. The `OpenMlsRustCrypto` storage is a
/// `HashMap<Vec<u8>, Vec<u8>>`; we persist it as a list of pairs because JSON
/// object keys must be strings (byte-array keys can't be JSON map keys).
#[derive(Serialize, Deserialize)]
struct PersistedEngine {
    storage: Vec<(Vec<u8>, Vec<u8>)>,
    identity: Vec<u8>,
    public_key: Vec<u8>,
}

/// Accord's MLS ciphersuite (ARCHITECTURE.md section 5.5):
/// `MLS_128_DHKEMX25519_AES128GCM_SHA256_Ed25519`.
const CIPHERSUITE: Ciphersuite = Ciphersuite::MLS_128_DHKEMX25519_AES128GCM_SHA256_Ed25519;

/// Result of feeding an inbound MLS message to [`MlsEngine::process_incoming`].
#[derive(Debug)]
pub enum DecryptOutcome {
    /// A decrypted application message (plaintext bytes).
    Application(Vec<u8>),
    /// A Commit was applied; the group advanced to a new epoch.
    CommitApplied,
    /// A proposal or other control message that produced no plaintext.
    Other,
}

/// One device's MLS state machine.
pub struct MlsEngine {
    provider: OpenMlsRustCrypto,
    signer: SignatureKeyPair,
    credential_with_key: CredentialWithKey,
    /// The raw credential identity (kept so the engine can be re-serialized).
    identity: Vec<u8>,
}

impl std::fmt::Debug for MlsEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MlsEngine")
            .field("identity_len", &self.identity.len())
            .finish_non_exhaustive()
    }
}

impl MlsEngine {
    /// Create a fresh engine whose MLS signing key **is** the device's Ed25519
    /// identity key (`signing`), and whose leaf credential is that key's public
    /// bytes. Binding the two means a peer who verifies a leaf's signature is
    /// verifying it against the very identity key the account is registered with -
    /// the credential isn't just a label, it's the key that signed.
    ///
    /// (Our ciphersuite's signature scheme is Ed25519, and OpenMLS stores an
    /// Ed25519 signing key as the 32-byte seed + 32-byte public key, which is
    /// exactly what [`IdentityKeyPair`] exposes, so the key transfers directly.)
    ///
    /// # Errors
    /// Returns [`MlsError`] if storing the signer fails.
    pub fn new(signing: &IdentityKeyPair) -> Result<Self, MlsError> {
        let provider = OpenMlsRustCrypto::default();
        let public = signing.public().to_bytes();
        let signer = SignatureKeyPair::from_raw(
            CIPHERSUITE.signature_algorithm(),
            signing.secret_bytes().to_vec(),
            public.to_vec(),
        );
        signer
            .store(provider.storage())
            .map_err(|e| MlsError::State(format!("store signer: {e}")))?;

        let credential_with_key = CredentialWithKey {
            credential: BasicCredential::new(public.to_vec()).into(),
            signature_key: signer.public().into(),
        };

        Ok(Self {
            provider,
            signer,
            credential_with_key,
            identity: public.to_vec(),
        })
    }

    /// The leaf credential identity (the signing key's public bytes).
    #[must_use]
    pub fn credential_identity(&self) -> &[u8] {
        &self.identity
    }

    /// Serialize the entire engine (all key + group state) to bytes for local
    /// storage or inclusion in the encrypted key backup (ARCHITECTURE.md section 6.5).
    ///
    /// # Errors
    /// Returns [`MlsError::State`] if the storage lock is poisoned or encoding fails.
    pub fn serialize(&self) -> Result<Vec<u8>, MlsError> {
        let map = self
            .provider
            .storage()
            .values
            .read()
            .map_err(|_| MlsError::State("storage lock poisoned".into()))?;
        let persisted = PersistedEngine {
            storage: map.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
            identity: self.identity.clone(),
            public_key: self.signer.to_public_vec(),
        };
        serde_json::to_vec(&persisted).map_err(|e| MlsError::State(e.to_string()))
    }

    /// Restore an engine previously produced by [`MlsEngine::serialize`].
    ///
    /// # Errors
    /// Returns [`MlsError::State`] if decoding fails or the signer is missing.
    pub fn from_serialized(bytes: &[u8]) -> Result<Self, MlsError> {
        let persisted: PersistedEngine =
            serde_json::from_slice(bytes).map_err(|e| MlsError::State(e.to_string()))?;

        let provider = OpenMlsRustCrypto::default();
        {
            let mut map = provider
                .storage()
                .values
                .write()
                .map_err(|_| MlsError::State("storage lock poisoned".into()))?;
            *map = persisted.storage.into_iter().collect::<HashMap<_, _>>();
        }

        let signer = SignatureKeyPair::read(
            provider.storage(),
            &persisted.public_key,
            CIPHERSUITE.signature_algorithm(),
        )
        .ok_or_else(|| MlsError::State("signer not found in restored storage".into()))?;

        let credential_with_key = CredentialWithKey {
            credential: BasicCredential::new(persisted.identity.clone()).into(),
            signature_key: persisted.public_key.into(),
        };

        Ok(Self {
            provider,
            signer,
            credential_with_key,
            identity: persisted.identity,
        })
    }

    /// Generate `count` KeyPackages (serialized wire bytes) for publishing to the
    /// server. Their private halves are kept in this engine's storage so it can
    /// later process Welcomes that reference them.
    ///
    /// # Errors
    /// Returns [`MlsError`] if KeyPackage construction or serialization fails.
    pub fn generate_key_packages(&self, count: usize) -> Result<Vec<Vec<u8>>, MlsError> {
        let mut out = Vec::with_capacity(count);
        for _ in 0..count {
            let bundle = KeyPackage::builder()
                .build(
                    CIPHERSUITE,
                    &self.provider,
                    &self.signer,
                    self.credential_with_key.clone(),
                )
                .map_err(|e| MlsError::Protocol(format!("build key package: {e}")))?;
            let bytes = bundle
                .key_package()
                .tls_serialize_detached()
                .map_err(|e| MlsError::Codec(e.to_string()))?;
            out.push(bytes);
        }
        Ok(out)
    }

    /// Create a new group identified by `group_id`, with this device as the only
    /// member.
    ///
    /// # Errors
    /// Returns [`MlsError`] if group creation fails.
    pub fn create_group(&mut self, group_id: &[u8]) -> Result<(), MlsError> {
        let config = MlsGroupCreateConfig::builder()
            .ciphersuite(CIPHERSUITE)
            .use_ratchet_tree_extension(true)
            .build();
        MlsGroup::new_with_group_id(
            &self.provider,
            &self.signer,
            &config,
            GroupId::from_slice(group_id),
            self.credential_with_key.clone(),
        )
        .map_err(|e| MlsError::Protocol(format!("create group: {e}")))?;
        Ok(())
    }

    /// Add a member to a group from their published KeyPackage bytes.
    ///
    /// Returns `(commit, welcome)` wire bytes: the Commit is relayed to existing
    /// members; the Welcome goes to the newly-added device.
    ///
    /// # Errors
    /// Returns [`MlsError`] on validation/protocol/codec failure.
    pub fn add_member(
        &mut self,
        group_id: &[u8],
        key_package: &[u8],
    ) -> Result<(Vec<u8>, Vec<u8>), MlsError> {
        let mut group = self.load_group(group_id)?;

        let kp_in = KeyPackageIn::tls_deserialize_exact(key_package)
            .map_err(|e| MlsError::Codec(e.to_string()))?;
        let kp: KeyPackage = kp_in
            .validate(self.provider.crypto(), ProtocolVersion::Mls10)
            .map_err(|e| MlsError::Protocol(format!("invalid key package: {e}")))?;

        let (commit, welcome, _group_info) = group
            .add_members(&self.provider, &self.signer, &[kp])
            .map_err(|e| MlsError::Protocol(format!("add member: {e}")))?;
        group
            .merge_pending_commit(&self.provider)
            .map_err(|e| MlsError::Protocol(format!("merge commit: {e}")))?;

        let commit_bytes = commit
            .to_bytes()
            .map_err(|e| MlsError::Codec(e.to_string()))?;
        let welcome_bytes = welcome
            .to_bytes()
            .map_err(|e| MlsError::Codec(e.to_string()))?;
        Ok((commit_bytes, welcome_bytes))
    }

    /// Join a group from a received Welcome, returning the new group's id.
    ///
    /// # Errors
    /// Returns [`MlsError`] if the Welcome is invalid or joining fails.
    pub fn join_from_welcome(&mut self, welcome: &[u8]) -> Result<Vec<u8>, MlsError> {
        let msg = MlsMessageIn::tls_deserialize_exact(welcome)
            .map_err(|e| MlsError::Codec(e.to_string()))?;
        let welcome = match msg.extract() {
            MlsMessageBodyIn::Welcome(w) => w,
            _ => return Err(MlsError::Protocol("message was not a Welcome".into())),
        };

        let join_config = MlsGroupJoinConfig::builder().build();
        let staged = StagedWelcome::new_from_welcome(&self.provider, &join_config, welcome, None)
            .map_err(|e| MlsError::Protocol(format!("staged welcome: {e}")))?;
        let group = staged
            .into_group(&self.provider)
            .map_err(|e| MlsError::Protocol(format!("join group: {e}")))?;

        Ok(group.group_id().as_slice().to_vec())
    }

    /// Encrypt an application message for a group's current epoch.
    ///
    /// # Errors
    /// Returns [`MlsError`] if the group is unknown or encryption fails.
    pub fn encrypt(&mut self, group_id: &[u8], plaintext: &[u8]) -> Result<Vec<u8>, MlsError> {
        let mut group = self.load_group(group_id)?;
        let out = group
            .create_message(&self.provider, &self.signer, plaintext)
            .map_err(|e| MlsError::Protocol(format!("encrypt: {e}")))?;
        out.to_bytes().map_err(|e| MlsError::Codec(e.to_string()))
    }

    /// Process an inbound MLS message (application message OR Commit) for a group.
    ///
    /// # Errors
    /// Returns [`MlsError`] if the group is unknown or processing fails.
    pub fn process_incoming(
        &mut self,
        group_id: &[u8],
        message: &[u8],
    ) -> Result<DecryptOutcome, MlsError> {
        let mut group = self.load_group(group_id)?;
        let msg = MlsMessageIn::tls_deserialize_exact(message)
            .map_err(|e| MlsError::Codec(e.to_string()))?;
        let protocol = msg
            .try_into_protocol_message()
            .map_err(|e| MlsError::Protocol(format!("not a protocol message: {e}")))?;
        let processed = group
            .process_message(&self.provider, protocol)
            .map_err(|e| MlsError::Protocol(format!("process message: {e}")))?;

        match processed.into_content() {
            ProcessedMessageContent::ApplicationMessage(app) => {
                Ok(DecryptOutcome::Application(app.into_bytes()))
            }
            ProcessedMessageContent::StagedCommitMessage(staged) => {
                group
                    .merge_staged_commit(&self.provider, *staged)
                    .map_err(|e| MlsError::Protocol(format!("merge staged commit: {e}")))?;
                Ok(DecryptOutcome::CommitApplied)
            }
            _ => Ok(DecryptOutcome::Other),
        }
    }

    /// Load a group from this engine's storage.
    fn load_group(&self, group_id: &[u8]) -> Result<MlsGroup, MlsError> {
        MlsGroup::load(self.provider.storage(), &GroupId::from_slice(group_id))
            .map_err(|e| MlsError::State(e.to_string()))?
            .ok_or(MlsError::UnknownGroup)
    }

    /// The credential identities (leaf signing-key public bytes) of every current
    /// member of a group, including this device. For a DM, the *other* identity is
    /// the peer's contact identity, so the UI can attribute the conversation.
    ///
    /// # Errors
    /// Returns [`MlsError`] if the group is unknown.
    pub fn group_member_identities(&self, group_id: &[u8]) -> Result<Vec<Vec<u8>>, MlsError> {
        let group = self.load_group(group_id)?;
        let ids = group
            .members()
            .filter_map(|m| BasicCredential::try_from(m.credential).ok())
            .map(|c| c.identity().to_vec())
            .collect();
        Ok(ids)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Full two-party flow: Alice creates a group, adds Bob (via his KeyPackage),
    /// Bob joins from the Welcome, and they exchange an encrypted message each
    /// way - proving the OpenMLS integration end-to-end.
    #[test]
    fn alice_and_bob_exchange_encrypted_messages() {
        let alice_key = IdentityKeyPair::generate();
        let bob_key = IdentityKeyPair::generate();
        let mut alice = MlsEngine::new(&alice_key).expect("alice engine");
        let mut bob = MlsEngine::new(&bob_key).expect("bob engine");

        // The MLS credential identity is bound to the signing key's public bytes.
        assert_eq!(alice.credential_identity(), alice_key.public().to_bytes());

        // Bob publishes a KeyPackage; Alice fetches it (here, directly).
        let bob_kp = bob.generate_key_packages(1).expect("bob kp").remove(0);

        // Alice creates the group and adds Bob.
        let group_id = b"test-group-id";
        alice.create_group(group_id).expect("create group");
        let (_commit, welcome) = alice.add_member(group_id, &bob_kp).expect("add bob");

        // Bob joins from the Welcome; he should learn the same group id.
        let bob_group_id = bob.join_from_welcome(&welcome).expect("bob joins");
        assert_eq!(bob_group_id, group_id);

        // Alice -> Bob.
        let ct = alice
            .encrypt(group_id, b"hello bob")
            .expect("alice encrypts");
        match bob.process_incoming(group_id, &ct).expect("bob decrypts") {
            DecryptOutcome::Application(pt) => assert_eq!(pt, b"hello bob"),
            other => panic!("expected application message, got {other:?}"),
        }

        // Bob -> Alice.
        let ct = bob.encrypt(group_id, b"hi alice").expect("bob encrypts");
        match alice
            .process_incoming(group_id, &ct)
            .expect("alice decrypts")
        {
            DecryptOutcome::Application(pt) => assert_eq!(pt, b"hi alice"),
            other => panic!("expected application message, got {other:?}"),
        }
    }

    /// An engine survives a serialize -> restore round-trip and can still decrypt
    /// messages for a group it was a member of (proves persistence + backup).
    #[test]
    fn engine_persists_and_restores() {
        let alice_key = IdentityKeyPair::generate();
        let bob_key = IdentityKeyPair::generate();
        let mut alice = MlsEngine::new(&alice_key).expect("alice");
        let mut bob = MlsEngine::new(&bob_key).expect("bob");

        let bob_kp = bob.generate_key_packages(1).expect("kp").remove(0);
        let group_id = b"persist-group";
        alice.create_group(group_id).expect("create");
        let (_commit, welcome) = alice.add_member(group_id, &bob_kp).expect("add");
        bob.join_from_welcome(&welcome).expect("join");

        // Round-trip Bob through serialization.
        let saved = bob.serialize().expect("serialize");
        let mut bob_restored = MlsEngine::from_serialized(&saved).expect("restore");

        // Restored Bob can still decrypt a new message from Alice.
        let ct = alice.encrypt(group_id, b"after restart").expect("encrypt");
        match bob_restored
            .process_incoming(group_id, &ct)
            .expect("decrypt")
        {
            DecryptOutcome::Application(pt) => assert_eq!(pt, b"after restart"),
            other => panic!("expected application message, got {other:?}"),
        }
    }
}
