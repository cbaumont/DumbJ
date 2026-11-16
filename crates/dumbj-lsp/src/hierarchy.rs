use std::path::Path;
use std::time::Duration;

use lsp_types::request::{
    CallHierarchyIncomingCalls, CallHierarchyOutgoingCalls, CallHierarchyPrepare,
    TypeHierarchyPrepare, TypeHierarchySubtypes, TypeHierarchySupertypes,
};
use lsp_types::{
    CallHierarchyIncomingCallsParams, CallHierarchyItem, CallHierarchyOutgoingCallsParams,
    CallHierarchyPrepareParams, PartialResultParams, Position, TypeHierarchyItem,
    TypeHierarchyPrepareParams, TypeHierarchySubtypesParams, TypeHierarchySupertypesParams,
    WorkDoneProgressParams,
};

use crate::goto::position_params;
use crate::{LspError, Server};

/// The type at `position` of `path`, as the root of a type hierarchy.
pub fn type_hierarchy_prepare(
    server: &Server,
    path: &Path,
    position: Position,
    timeout: Duration,
) -> Result<Vec<TypeHierarchyItem>, LspError> {
    let params = TypeHierarchyPrepareParams {
        text_document_position_params: position_params(path, position),
        work_done_progress_params: WorkDoneProgressParams::default(),
    };
    let items = server
        .request::<TypeHierarchyPrepare>(params)
        .wait(timeout)?;
    Ok(items.unwrap_or_default())
}

pub fn supertypes(
    server: &Server,
    item: TypeHierarchyItem,
    timeout: Duration,
) -> Result<Vec<TypeHierarchyItem>, LspError> {
    let params = TypeHierarchySupertypesParams {
        item,
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: PartialResultParams::default(),
    };
    let items = server
        .request::<TypeHierarchySupertypes>(params)
        .wait(timeout)?;
    Ok(items.unwrap_or_default())
}

pub fn subtypes(
    server: &Server,
    item: TypeHierarchyItem,
    timeout: Duration,
) -> Result<Vec<TypeHierarchyItem>, LspError> {
    let params = TypeHierarchySubtypesParams {
        item,
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: PartialResultParams::default(),
    };
    let items = server
        .request::<TypeHierarchySubtypes>(params)
        .wait(timeout)?;
    Ok(items.unwrap_or_default())
}

/// The callable at `position` of `path`, as the root of a call hierarchy.
pub fn call_hierarchy_prepare(
    server: &Server,
    path: &Path,
    position: Position,
    timeout: Duration,
) -> Result<Vec<CallHierarchyItem>, LspError> {
    let params = CallHierarchyPrepareParams {
        text_document_position_params: position_params(path, position),
        work_done_progress_params: WorkDoneProgressParams::default(),
    };
    let items = server
        .request::<CallHierarchyPrepare>(params)
        .wait(timeout)?;
    Ok(items.unwrap_or_default())
}

/// The callers of `item`; the call-site ranges are dropped.
pub fn incoming_calls(
    server: &Server,
    item: CallHierarchyItem,
    timeout: Duration,
) -> Result<Vec<CallHierarchyItem>, LspError> {
    let params = CallHierarchyIncomingCallsParams {
        item,
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: PartialResultParams::default(),
    };
    let calls = server
        .request::<CallHierarchyIncomingCalls>(params)
        .wait(timeout)?;
    Ok(calls
        .unwrap_or_default()
        .into_iter()
        .map(|call| call.from)
        .collect())
}

/// The callees of `item`; the call-site ranges are dropped.
pub fn outgoing_calls(
    server: &Server,
    item: CallHierarchyItem,
    timeout: Duration,
) -> Result<Vec<CallHierarchyItem>, LspError> {
    let params = CallHierarchyOutgoingCallsParams {
        item,
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: PartialResultParams::default(),
    };
    let calls = server
        .request::<CallHierarchyOutgoingCalls>(params)
        .wait(timeout)?;
    Ok(calls
        .unwrap_or_default()
        .into_iter()
        .map(|call| call.to)
        .collect())
}
