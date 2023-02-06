// RGB Core Library: consensus layer for RGB smart contracts.
//
// SPDX-License-Identifier: Apache-2.0
//
// Written in 2019-2023 by
//     Dr Maxim Orlovsky <orlovsky@lnp-bp.org>
//
// Copyright (C) 2019-2023 LNP/BP Standards Association. All rights reserved.
// Copyright (C) 2019-2023 Dr Maxim Orlovsky. All rights reserved.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::num::ParseIntError;
use std::str::FromStr;

use amplify::confinement::{TinyOrdMap, TinyOrdSet, TinyVec};
use amplify::hex::{FromHex, ToHex};
use amplify::{hex, AsAny, Bytes32, RawArray, Wrapper};
use baid58::{Baid58ParseError, FromBaid58, ToBaid58};
use bp::seals::txout::TxoSeal;
use bp::{Chain, Outpoint, Txid};
use commit_verify::{mpc, CommitStrategy, CommitmentId};

use super::{seal, ConcealSeals, ConcealState, ConfidentialDataError, Metadata, TypedAssignments};
use crate::schema::{
    self, ExtensionType, FieldType, NodeSubtype, NodeType, OwnedRightType, PublicRightType,
    SchemaId, TransitionType,
};
use crate::LIB_NAME_RGB;

/// RGB contract node output pointer, defined by the node ID and output number.
#[derive(Copy, Clone, Ord, PartialOrd, Eq, PartialEq, Hash, Debug, Display)]
#[derive(StrictType, StrictDumb, StrictEncode, StrictDecode)]
#[strict_type(lib = LIB_NAME_RGB)]
#[display("{node_id}/{ty}/{no}")]
pub struct NodeOutpoint {
    pub node_id: NodeId,
    pub ty: OwnedRightType,
    pub no: u16,
}

impl NodeOutpoint {
    pub fn new(node_id: NodeId, ty: u16, no: u16) -> NodeOutpoint {
        NodeOutpoint { node_id, ty, no }
    }
}

#[derive(Clone, Eq, PartialEq, Debug, Display, Error, From)]
#[display(inner)]
pub enum OutpointParseError {
    #[from]
    InvalidNodeId(amplify::hex::Error),

    InvalidType(ParseIntError),

    InvalidOutputNo(ParseIntError),

    /// invalid node outpoint format ('{0}')
    #[display(doc_comments)]
    WrongFormat(String),
}

impl FromStr for NodeOutpoint {
    type Err = OutpointParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut split = s.split('/');
        match (split.next(), split.next(), split.next(), split.next()) {
            (Some(node_id), Some(ty), Some(no), None) => Ok(NodeOutpoint {
                node_id: node_id.parse()?,
                ty: ty.parse().map_err(OutpointParseError::InvalidType)?,
                no: no.parse().map_err(OutpointParseError::InvalidOutputNo)?,
            }),
            _ => Err(OutpointParseError::WrongFormat(s.to_owned())),
        }
    }
}

pub type PublicRights = TinyOrdSet<schema::PublicRightType>;
pub type OwnedRights = TinyOrdMap<schema::OwnedRightType, TypedAssignments>;
pub type ParentOwnedRights = TinyOrdMap<NodeId, TinyOrdMap<schema::OwnedRightType, TinyVec<u16>>>;
pub type ParentPublicRights = TinyOrdMap<NodeId, TinyOrdSet<schema::PublicRightType>>;

/// Unique node (genesis, extensions & state transition) identifier equivalent
/// to the commitment hash
#[derive(Wrapper, Copy, Clone, Ord, PartialOrd, Eq, PartialEq, Hash, Debug, Display, From)]
#[wrapper(Deref, BorrowSlice, Hex, Index, RangeOps)]
#[display(Self::to_hex)]
#[derive(StrictType, StrictDumb, StrictEncode, StrictDecode)]
#[strict_type(lib = LIB_NAME_RGB)]
#[cfg_attr(
    feature = "serde",
    derive(Serialize, Deserialize),
    serde(crate = "serde_crate", transparent)
)]
pub struct NodeId(
    #[from]
    #[from([u8; 32])]
    Bytes32,
);

impl FromStr for NodeId {
    type Err = hex::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> { Self::from_hex(s) }
}

/// Unique contract identifier equivalent to the contract genesis commitment
#[derive(Wrapper, Copy, Clone, Ord, PartialOrd, Eq, PartialEq, Hash, Debug, Display, From)]
#[wrapper(Deref, BorrowSlice, Hex, Index, RangeOps)]
#[display(Self::to_baid58)]
#[derive(StrictType, StrictDumb, StrictEncode, StrictDecode)]
#[strict_type(lib = LIB_NAME_RGB)]
#[cfg_attr(
    feature = "serde",
    derive(Serialize, Deserialize),
    serde(crate = "serde_crate", transparent)
)]
pub struct ContractId(
    #[from]
    #[from([u8; 32])]
    Bytes32,
);

impl ToBaid58<32> for ContractId {
    const HRI: &'static str = "rgb";
    fn to_baid58_payload(&self) -> [u8; 32] { self.to_raw_array() }
}
impl FromBaid58<32> for ContractId {}

impl FromStr for ContractId {
    type Err = Baid58ParseError;
    fn from_str(s: &str) -> Result<Self, Self::Err> { Self::from_baid58_str(s) }
}

impl From<mpc::ProtocolId> for ContractId {
    fn from(id: mpc::ProtocolId) -> Self { ContractId(id.into_inner()) }
}

impl From<ContractId> for mpc::ProtocolId {
    fn from(id: ContractId) -> Self { mpc::ProtocolId::from_inner(id.into_inner()) }
}

/// RGB contract node API, defined as trait
///
/// Implemented by all contract node types (see [`NodeType`]):
/// - Genesis ([`Genesis`])
/// - State transitions ([`Transitions`])
/// - Public state extensions ([`Extensions`])
pub trait Node: AsAny {
    /// Returns type of the node (see [`NodeType`]). Unfortunately, this can't
    /// be just a const, since it will break our ability to convert concrete
    /// `Node` types into `&dyn Node` (entities implementing traits with const
    /// definitions can't be made into objects)
    fn node_type(&self) -> NodeType;

    /// Returns full contract node type information
    fn subtype(&self) -> NodeSubtype;

    /// Returns [`NodeId`], which is a hash of this node commitment
    /// serialization
    fn node_id(&self) -> NodeId;

    /// Returns [`Option::Some`]`(`[`ContractId`]`)`, which is a hash of
    /// genesis.
    /// - For genesis node, this hash is byte-equal to [`NodeId`] (however
    ///   displayed in a reverse manner, to introduce semantical distinction)
    /// - For extension node function returns id of the genesis, to which this
    ///   node commits to
    /// - For state transition function returns [`Option::None`], since they do
    ///   not keep this information; it must be deduced through state transition
    ///   graph
    fn contract_id(&self) -> Option<ContractId>;

    /// Returns [`Option::Some`]`(`[`TransitionType`]`)` for transitions or
    /// [`Option::None`] for genesis and extension node types
    fn transition_type(&self) -> Option<TransitionType>;

    /// Returns [`Option::Some`]`(`[`ExtensionType`]`)` for extension nodes or
    /// [`Option::None`] for genesis and trate transitions
    fn extension_type(&self) -> Option<ExtensionType>;

    /// Returns reference to a full set of metadata (in form of [`Metadata`]
    /// wrapper structure) for the contract node.
    fn metadata(&self) -> &Metadata;

    /// Returns reference to information about the owned rights in form of
    /// [`ParentOwnedRights`] wrapper structure which this node updates with
    /// state transition ("parent owned rights").
    ///
    /// This is always an empty `Vec` for [`Genesis`] and [`Extension`] node
    /// types.
    fn parent_owned_rights(&self) -> &ParentOwnedRights;

    /// Returns reference to information about the public rights (in form of
    /// [`ParentPublicRights`] wrapper structure), defined with "parent" state
    /// extensions (i.e. those finalized with the current state transition) or
    /// referenced by another state extension, which this node updates
    /// ("parent public rights").
    ///
    /// This is always an empty `Vec` for [`Genesis`].
    fn parent_public_rights(&self) -> &ParentPublicRights;
    fn owned_rights(&self) -> &OwnedRights;
    fn owned_rights_mut(&mut self) -> &mut OwnedRights;
    fn public_rights(&self) -> &PublicRights;
    fn public_rights_mut(&mut self) -> &mut PublicRights;

    #[inline]
    fn field_types(&self) -> Vec<FieldType> { self.metadata().keys().copied().collect() }

    #[inline]
    fn parent_public_right_types(&self) -> Vec<PublicRightType> {
        self.parent_public_rights()
            .values()
            .flat_map(|v| v.iter())
            .copied()
            .collect()
    }

    #[inline]
    fn parent_by_public_right_type(&self, t: PublicRightType) -> Vec<NodeId> {
        self.parent_public_rights()
            .iter()
            .filter(|(_, t2)| t2.contains(&t))
            .map(|(node_id, _)| *node_id)
            .collect()
    }

    /// For genesis and public state extensions always returns an empty list.
    /// While public state extension do have parent nodes, they do not contain
    /// indexed rights.
    #[inline]
    fn parent_outputs(&self) -> Vec<NodeOutpoint> {
        self.parent_owned_rights()
            .iter()
            .flat_map(|(node_id, map)| {
                let node_id = *node_id;
                map.iter()
                    .flat_map(|(ty, vec)| vec.iter().map(|no| (*ty, *no)))
                    .map(move |(ty, no)| NodeOutpoint { node_id, ty, no })
            })
            .collect()
    }

    #[inline]
    fn parent_outputs_by_type(&self, t: OwnedRightType) -> Vec<NodeOutpoint> {
        self.parent_outputs_by_types(&[t])
    }

    fn parent_outputs_by_types(&self, types: &[OwnedRightType]) -> Vec<NodeOutpoint> {
        self.parent_owned_rights()
            .iter()
            .flat_map(|(node_id, map)| {
                let node_id = *node_id;
                map.iter()
                    .filter(|(t, _)| types.contains(*t))
                    .flat_map(|(ty, vec)| vec.iter().map(|no| (*ty, *no)))
                    .map(move |(ty, no)| NodeOutpoint { node_id, ty, no })
            })
            .collect()
    }

    #[inline]
    fn parent_owned_right_types(&self) -> Vec<OwnedRightType> {
        self.parent_owned_rights()
            .values()
            .flat_map(|v| v.keys())
            .copied()
            .collect()
    }

    #[inline]
    fn owned_right_types(&self) -> BTreeSet<OwnedRightType> {
        self.owned_rights().keys().cloned().collect()
    }

    #[inline]
    fn owned_rights_by_type(&self, t: OwnedRightType) -> Option<&TypedAssignments> {
        self.owned_rights()
            .iter()
            .find_map(|(t2, a)| if *t2 == t { Some(a) } else { None })
    }

    #[inline]
    fn to_confiential_seals(&self) -> Vec<seal::Confidential> {
        self.owned_rights()
            .iter()
            .flat_map(|(_, assignment)| assignment.to_confidential_seals())
            .collect()
    }

    #[inline]
    fn revealed_seals(&self) -> Result<Vec<seal::Revealed>, ConfidentialDataError> {
        let unfiltered = self
            .owned_rights()
            .iter()
            .map(|(_, assignment)| assignment.revealed_seals())
            .collect::<Vec<_>>();
        if unfiltered.contains(&Err(ConfidentialDataError)) {
            return Err(ConfidentialDataError);
        }
        Ok(unfiltered
            .into_iter()
            .filter_map(Result::ok)
            .flat_map(Vec::into_iter)
            .collect())
    }

    #[inline]
    fn revealed_seals_by_type(
        &self,
        assignment_type: OwnedRightType,
    ) -> Result<Vec<seal::Revealed>, ConfidentialDataError> {
        Ok(self
            .owned_rights_by_type(assignment_type)
            .map(TypedAssignments::revealed_seals)
            .transpose()?
            .unwrap_or_default())
    }

    #[inline]
    fn filter_revealed_seals(&self) -> Vec<seal::Revealed> {
        self.owned_rights()
            .iter()
            .flat_map(|(_, assignment)| assignment.filter_revealed_seals())
            .collect()
    }

    #[inline]
    fn filter_revealed_seals_by_type(
        &self,
        assignment_type: OwnedRightType,
    ) -> Vec<seal::Revealed> {
        self.owned_rights_by_type(assignment_type)
            .map(TypedAssignments::filter_revealed_seals)
            .unwrap_or_else(Vec::new)
    }

    fn node_outputs(&self, witness_txid: Txid) -> BTreeMap<NodeOutpoint, Outpoint> {
        let node_id = self.node_id();
        let mut res: BTreeMap<NodeOutpoint, Outpoint> = bmap! {};
        for (ty, assignments) in self.owned_rights() {
            for (seal, node_output) in assignments.revealed_seal_outputs() {
                let outpoint = seal.outpoint_or(witness_txid);
                let node_outpoint = NodeOutpoint::new(node_id, *ty, node_output);
                res.insert(node_outpoint, outpoint);
            }
        }
        res
    }
}

#[derive(Clone, PartialEq, Eq, Hash, Debug, AsAny)]
#[derive(StrictType, StrictDumb, StrictEncode, StrictDecode)]
#[strict_type(lib = LIB_NAME_RGB)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize), serde(crate = "serde_crate"))]
pub struct Genesis {
    schema_id: SchemaId,
    chain: Chain,
    metadata: Metadata,
    owned_rights: OwnedRights,
    public_rights: PublicRights,
}

#[derive(Clone, PartialEq, Eq, Hash, Debug, AsAny)]
#[derive(StrictType, StrictDumb, StrictEncode, StrictDecode)]
#[strict_type(lib = LIB_NAME_RGB)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize), serde(crate = "serde_crate"))]
pub struct Extension {
    extension_type: ExtensionType,
    contract_id: ContractId,
    metadata: Metadata,
    owned_rights: OwnedRights,
    parent_public_rights: ParentPublicRights,
    public_rights: PublicRights,
}

#[derive(Clone, PartialEq, Eq, Hash, Debug, AsAny)]
#[derive(StrictType, StrictDumb, StrictEncode, StrictDecode)]
#[strict_type(lib = LIB_NAME_RGB)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize), serde(crate = "serde_crate"))]
pub struct Transition {
    transition_type: TransitionType,
    metadata: Metadata,
    parent_owned_rights: ParentOwnedRights,
    owned_rights: OwnedRights,
    public_rights: PublicRights,
}

// TODO: Remove after TransitionBundling refactoring
impl Ord for Transition {
    fn cmp(&self, other: &Self) -> Ordering { self.node_id().cmp(&other.node_id()) }
}

impl PartialOrd for Transition {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> { Some(self.cmp(other)) }
}

impl ConcealState for Genesis {
    fn conceal_state_except(&mut self, seals: &[seal::Confidential]) -> usize {
        let mut count = 0;
        for (_, assignment) in self.owned_rights_mut().keyed_values_mut() {
            count += assignment.conceal_state_except(seals);
        }
        count
    }
}

impl ConcealState for Extension {
    fn conceal_state_except(&mut self, seals: &[seal::Confidential]) -> usize {
        let mut count = 0;
        for (_, assignment) in self.owned_rights_mut().keyed_values_mut() {
            count += assignment.conceal_state_except(seals);
        }
        count
    }
}

impl ConcealState for Transition {
    fn conceal_state_except(&mut self, seals: &[seal::Confidential]) -> usize {
        let mut count = 0;
        for (_, assignment) in self.owned_rights_mut().keyed_values_mut() {
            count += assignment.conceal_state_except(seals);
        }
        count
    }
}

impl ConcealSeals for Genesis {
    fn conceal_seals(&mut self, seals: &[seal::Confidential]) -> usize {
        let mut count = 0;
        for (_, assignment) in self.owned_rights_mut().keyed_values_mut() {
            count += assignment.conceal_seals(seals);
        }
        count
    }
}

impl ConcealSeals for Transition {
    fn conceal_seals(&mut self, seals: &[seal::Confidential]) -> usize {
        let mut count = 0;
        for (_, assignment) in self.owned_rights_mut().keyed_values_mut() {
            count += assignment.conceal_seals(seals);
        }
        count
    }
}

impl ConcealSeals for Extension {
    fn conceal_seals(&mut self, seals: &[seal::Confidential]) -> usize {
        let mut count = 0;
        for (_, assignment) in self.owned_rights_mut().keyed_values_mut() {
            count += assignment.conceal_seals(seals);
        }
        count
    }
}

impl CommitStrategy for Genesis {
    // TODO: Use merklization
    type Strategy = commit_verify::strategies::Strict;
}

impl CommitmentId for Genesis {
    const TAG: [u8; 32] = *b"urn:lnpbp:rgb:genesis:v01#202302";
    type Id = ContractId;
}

impl CommitStrategy for Transition {
    // TODO: Use merklization
    type Strategy = commit_verify::strategies::Strict;
}

impl CommitmentId for Transition {
    const TAG: [u8; 32] = *b"urn:lnpbp:rgb:transition:v01#32A";
    type Id = NodeId;
}

impl CommitStrategy for Extension {
    // TODO: Use merklization
    type Strategy = commit_verify::strategies::Strict;
}

impl CommitmentId for Extension {
    const TAG: [u8; 32] = *b"urn:lnpbp:rgb:extension:v01#2023";
    type Id = NodeId;
}

impl Node for Genesis {
    #[inline]
    fn node_type(&self) -> NodeType { NodeType::Genesis }

    #[inline]
    fn subtype(&self) -> NodeSubtype { NodeSubtype::Genesis }

    #[inline]
    fn node_id(&self) -> NodeId { NodeId(self.commitment_id().into_inner()) }

    #[inline]
    fn contract_id(&self) -> Option<ContractId> {
        Some(ContractId::from_inner(self.node_id().into_inner()))
    }

    #[inline]
    fn transition_type(&self) -> Option<TransitionType> { None }

    #[inline]
    fn extension_type(&self) -> Option<ExtensionType> { None }

    #[inline]
    fn parent_owned_rights(&self) -> &ParentOwnedRights {
        panic!("genesis can't close previous single-use-seals")
    }

    #[inline]
    fn parent_public_rights(&self) -> &ParentPublicRights {
        panic!("genesis can't extend previous state")
    }

    #[inline]
    fn metadata(&self) -> &Metadata { &self.metadata }

    #[inline]
    fn owned_rights(&self) -> &OwnedRights { &self.owned_rights }

    #[inline]
    fn owned_rights_mut(&mut self) -> &mut OwnedRights { &mut self.owned_rights }

    #[inline]
    fn public_rights(&self) -> &PublicRights { &self.public_rights }

    #[inline]
    fn public_rights_mut(&mut self) -> &mut PublicRights { &mut self.public_rights }
}

impl Node for Extension {
    #[inline]
    fn node_type(&self) -> NodeType { NodeType::StateExtension }

    #[inline]
    fn subtype(&self) -> NodeSubtype { NodeSubtype::StateExtension(self.extension_type) }

    #[inline]
    fn node_id(&self) -> NodeId { self.commitment_id() }

    #[inline]
    fn contract_id(&self) -> Option<ContractId> { Some(self.contract_id) }

    #[inline]
    fn transition_type(&self) -> Option<TransitionType> { None }

    #[inline]
    fn extension_type(&self) -> Option<ExtensionType> { Some(self.extension_type) }

    #[inline]
    fn parent_owned_rights(&self) -> &ParentOwnedRights {
        panic!("extension can't close previous single-use-seals")
    }

    #[inline]
    fn parent_public_rights(&self) -> &ParentPublicRights { &self.parent_public_rights }

    #[inline]
    fn metadata(&self) -> &Metadata { &self.metadata }

    #[inline]
    fn owned_rights(&self) -> &OwnedRights { &self.owned_rights }

    #[inline]
    fn owned_rights_mut(&mut self) -> &mut OwnedRights { &mut self.owned_rights }

    #[inline]
    fn public_rights(&self) -> &PublicRights { &self.public_rights }

    #[inline]
    fn public_rights_mut(&mut self) -> &mut PublicRights { &mut self.public_rights }
}

impl Node for Transition {
    #[inline]
    fn node_type(&self) -> NodeType { NodeType::StateTransition }

    #[inline]
    fn subtype(&self) -> NodeSubtype { NodeSubtype::StateTransition(self.transition_type) }

    #[inline]
    fn node_id(&self) -> NodeId { self.commitment_id() }

    #[inline]
    fn contract_id(&self) -> Option<ContractId> { None }

    #[inline]
    fn transition_type(&self) -> Option<TransitionType> { Some(self.transition_type) }

    #[inline]
    fn extension_type(&self) -> Option<ExtensionType> { None }

    #[inline]
    fn parent_owned_rights(&self) -> &ParentOwnedRights { &self.parent_owned_rights }

    #[inline]
    fn parent_public_rights(&self) -> &ParentPublicRights {
        panic!("state transitions can't extend previous state")
    }

    #[inline]
    fn metadata(&self) -> &Metadata { &self.metadata }

    #[inline]
    fn owned_rights(&self) -> &OwnedRights { &self.owned_rights }

    #[inline]
    fn owned_rights_mut(&mut self) -> &mut OwnedRights { &mut self.owned_rights }

    #[inline]
    fn public_rights(&self) -> &PublicRights { &self.public_rights }

    #[inline]
    fn public_rights_mut(&mut self) -> &mut PublicRights { &mut self.public_rights }
}

impl Genesis {
    pub fn with(
        schema_id: SchemaId,
        chain: Chain,
        metadata: Metadata,
        owned_rights: OwnedRights,
        public_rights: PublicRights,
    ) -> Self {
        Self {
            schema_id,
            chain,
            metadata,
            owned_rights,
            public_rights,
        }
    }

    #[inline]
    pub fn contract_id(&self) -> ContractId { ContractId::from_inner(self.node_id().into_inner()) }

    #[inline]
    pub fn schema_id(&self) -> SchemaId { self.schema_id }

    #[inline]
    pub fn chain(&self) -> &Chain { &self.chain }
}

impl Extension {
    pub fn with(
        extension_type: ExtensionType,
        contract_id: ContractId,
        metadata: Metadata,
        owned_rights: OwnedRights,
        parent_public_rights: ParentPublicRights,
        public_rights: PublicRights,
    ) -> Self {
        Self {
            extension_type,
            contract_id,
            metadata,
            parent_public_rights,
            owned_rights,
            public_rights,
        }
    }
}

impl Transition {
    pub fn with(
        transition_type: impl Into<schema::TransitionType>,
        metadata: Metadata,
        owned_rights: OwnedRights,
        public_rights: PublicRights,
        parent_owned_rights: ParentOwnedRights,
    ) -> Self {
        Self {
            transition_type: transition_type.into(),
            metadata,
            parent_owned_rights,
            owned_rights,
            public_rights,
        }
    }

    pub fn transition_type(&self) -> schema::TransitionType { self.transition_type }
}