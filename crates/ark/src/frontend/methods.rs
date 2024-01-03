//
// methods.rs
//
// Copyright (C) 2023 by Posit Software, PBC
//
//

use amalthea::comm::frontend_comm::FrontendFrontendRpcRequest;
use libR_shim::SEXP;

use crate::interface::RMain;

#[harp::register]
pub unsafe extern "C" fn ps_context_active_document() -> anyhow::Result<SEXP> {
    let main = RMain::get();
    let result = main.call_frontend_method(FrontendFrontendRpcRequest::LastActiveEditorContext)?;
    Ok(result.sexp)
}
