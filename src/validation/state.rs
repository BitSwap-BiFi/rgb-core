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

use core::any::Any;

use amplify::AsAny;
use commit_verify::Conceal;

use crate::contract::owned_state::{
    AttachmentStrategy, DeclarativeStrategy, HashStrategy, PedersenStrategy,
};
use crate::schema::OwnedRightType;
use crate::{validation, Assignment, NodeId, State, StateSchema};

impl StateSchema {
    pub fn validate<STATE>(
        &self,
        // type_system: &TypeSystem,
        node_id: &NodeId,
        assignment_id: OwnedRightType,
        data: &Assignment<STATE>,
    ) -> validation::Status
    where
        STATE: State,
        STATE::Confidential: PartialEq + Eq,
        STATE::Confidential: From<<STATE::Revealed as Conceal>::Concealed>,
    {
        let mut status = validation::Status::new();
        match data {
            Assignment::Confidential { state, .. } |
            Assignment::ConfidentialState { state, .. } => {
                let a: &dyn Any = state.as_any();
                match self {
                    StateSchema::Declarative => {
                        if a.downcast_ref::<<DeclarativeStrategy as State>::Confidential>()
                            .is_none()
                        {
                            status.add_failure(validation::Failure::SchemaMismatchedStateType(
                                assignment_id,
                            ));
                        }
                    }
                    StateSchema::Arithmetic(_) => {
                        if let Some(value) =
                            a.downcast_ref::<<PedersenStrategy as State>::Confidential>()
                        {
                            // [SECURITY-CRITICAL]: Bulletproofs validation
                            if let Err(err) = value.verify_range_proof() {
                                status.add_failure(validation::Failure::InvalidBulletproofs(
                                    *node_id,
                                    assignment_id,
                                    err.to_string(),
                                ));
                            }
                        } else {
                            status.add_failure(validation::Failure::SchemaMismatchedStateType(
                                assignment_id,
                            ));
                        }

                        // TODO: When other homomorphic formats will be added,
                        //       add information to the status like with hashed
                        //       data below
                    }
                    StateSchema::Structured(_) => {
                        match a.downcast_ref::<<HashStrategy as State>::Confidential>() {
                            None => {
                                status.add_failure(validation::Failure::SchemaMismatchedStateType(
                                    assignment_id,
                                ));
                            }
                            Some(_) => {
                                status.add_info(
                                    validation::Info::UncheckableConfidentialStateData(
                                        *node_id,
                                        assignment_id,
                                    ),
                                );
                            }
                        }
                    }
                    StateSchema::Attachment => {
                        if a.downcast_ref::<<AttachmentStrategy as State>::Confidential>()
                            .is_none()
                        {
                            status.add_failure(validation::Failure::SchemaMismatchedStateType(
                                assignment_id,
                            ));
                        }
                    }
                }
            }
            Assignment::Revealed { state, .. } | Assignment::ConfidentialSeal { state, .. } => {
                let a: &dyn Any = state.as_any();
                match self {
                    StateSchema::Declarative => {
                        if a.downcast_ref::<<DeclarativeStrategy as State>::Revealed>()
                            .is_none()
                        {
                            status.add_failure(validation::Failure::SchemaMismatchedStateType(
                                assignment_id,
                            ));
                        }
                    }
                    StateSchema::Arithmetic(_format) => {
                        if a.downcast_ref::<<PedersenStrategy as State>::Revealed>()
                            .is_none()
                        {
                            status.add_failure(validation::Failure::SchemaMismatchedStateType(
                                assignment_id,
                            ));
                        }
                        // TODO #15: When other homomorphic formats will be
                        // added,       add type check
                        // like with hashed data below
                    }
                    StateSchema::Structured(_semid) => {
                        match a.downcast_ref::<<HashStrategy as State>::Revealed>() {
                            None => {
                                status.add_failure(validation::Failure::SchemaMismatchedStateType(
                                    assignment_id,
                                ));
                            }
                            Some(_data) => {
                                // TODO: #137 run strict type validation
                            }
                        }
                    }
                    StateSchema::Attachment => {
                        if a.downcast_ref::<<AttachmentStrategy as State>::Revealed>()
                            .is_none()
                        {
                            status.add_failure(validation::Failure::SchemaMismatchedStateType(
                                assignment_id,
                            ));
                        }
                    }
                }
            }
        }
        status
    }
}