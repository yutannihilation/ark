/*
 * r_request.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use amalthea::wire::execute_request::ExecuteRequest;
use amalthea::wire::execute_response::ExecuteResponse;
use std::sync::mpsc::Sender;

/// Represents requests to the primary R execution thread.
pub enum RRequest {
    /// Fulfill an execution request from the front end, producing either a
    /// Reply or an Exception
    ExecuteCode(ExecuteRequest, Sender<ExecuteResponse>),

    /// Shut down the R execution thread
    Shutdown(bool),
}
