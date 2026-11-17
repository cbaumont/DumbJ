use std::path::Path;
use std::time::Duration;

use lsp_types::request::{
    CallHierarchyIncomingCalls, CallHierarchyOutgoingCalls, CallHierarchyPrepare, Request,
};
use lsp_types::{
    CallHierarchyIncomingCallsParams, CallHierarchyItem, CallHierarchyOutgoingCallsParams,
    CallHierarchyPrepareParams, PartialResultParams, Position, TypeHierarchyPrepareParams,
    WorkDoneProgressParams,
};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::goto::position_params;
use crate::{LspError, Server};

/// A type hierarchy item. lsp-types 0.97 declares `tags` as a single tag, so jdtls's
/// `"tags": []` would not decode into its `TypeHierarchyItem`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TypeItem {
    pub name: String,
    pub kind: lsp_types::SymbolKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<lsp_types::SymbolTag>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    pub uri: lsp_types::Uri,
    pub range: lsp_types::Range,
    pub selection_range: lsp_types::Range,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct ItemParams {
    item: TypeItem,
}

#[derive(Debug)]
pub(crate) enum Prepare {}
impl Request for Prepare {
    type Params = TypeHierarchyPrepareParams;
    type Result = Option<Vec<TypeItem>>;
    const METHOD: &'static str = "textDocument/prepareTypeHierarchy";
}

#[derive(Debug)]
pub(crate) enum Supertypes {}
impl Request for Supertypes {
    type Params = ItemParams;
    type Result = Option<Vec<TypeItem>>;
    const METHOD: &'static str = "typeHierarchy/supertypes";
}

#[derive(Debug)]
pub(crate) enum Subtypes {}
impl Request for Subtypes {
    type Params = ItemParams;
    type Result = Option<Vec<TypeItem>>;
    const METHOD: &'static str = "typeHierarchy/subtypes";
}

pub fn type_hierarchy_prepare(
    server: &Server,
    path: &Path,
    position: Position,
    timeout: Duration,
) -> Result<Vec<TypeItem>, LspError> {
    let params = TypeHierarchyPrepareParams {
        text_document_position_params: position_params(path, position),
        work_done_progress_params: WorkDoneProgressParams::default(),
    };
    let items = server.request::<Prepare>(params).wait(timeout)?;
    Ok(items.unwrap_or_default())
}

pub fn supertypes(
    server: &Server,
    item: TypeItem,
    timeout: Duration,
) -> Result<Vec<TypeItem>, LspError> {
    let params = ItemParams { item };
    let items = server.request::<Supertypes>(params).wait(timeout)?;
    Ok(items.unwrap_or_default())
}

pub fn subtypes(
    server: &Server,
    item: TypeItem,
    timeout: Duration,
) -> Result<Vec<TypeItem>, LspError> {
    let params = ItemParams { item };
    let items = server.request::<Subtypes>(params).wait(timeout)?;
    Ok(items.unwrap_or_default())
}

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
