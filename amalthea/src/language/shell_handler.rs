/*
 * shell_handler.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use crate::wire::comm_info_reply::CommInfoReply;
use crate::wire::comm_info_request::CommInfoRequest;
use crate::wire::complete_reply::CompleteReply;
use crate::wire::complete_request::CompleteRequest;
use crate::wire::exception::Exception;
use crate::wire::execute_reply::ExecuteReply;
use crate::wire::execute_reply_exception::ExecuteReplyException;
use crate::wire::execute_request::ExecuteRequest;
use crate::wire::is_complete_reply::IsCompleteReply;
use crate::wire::is_complete_request::IsCompleteRequest;
use crate::wire::kernel_info_reply::KernelInfoReply;
use crate::wire::kernel_info_request::KernelInfoRequest;

pub trait ShellHandler: Send {
    /// Handles a request for information about the kernel.
    ///
    /// Docs: https://jupyter-client.readthedocs.io/en/stable/messaging.html#kernel-info
    fn handle_info_request(&self, req: KernelInfoRequest) -> Result<KernelInfoReply, Exception>;

    /// Handles a request to test a fragment of code to see whether it is a complete expression.
    ///
    /// Docs: https://jupyter-client.readthedocs.io/en/stable/messaging.html#code-completeness
    fn handle_is_complete_request(
        &self,
        req: IsCompleteRequest,
    ) -> Result<IsCompleteReply, Exception>;

    /// Handles a request to execute code.
    ///
    /// Docs: https://jupyter-client.readthedocs.io/en/stable/messaging.html#execute
    fn handle_execute_request(
        &mut self,
        req: ExecuteRequest,
    ) -> Result<ExecuteReply, ExecuteReplyException>;

    /// Handles a request to provide completions for the given code fragment.
    ///
    /// Docs: https://jupyter-client.readthedocs.io/en/stable/messaging.html#completion
    fn handle_complete_request(&self, req: CompleteRequest) -> Result<CompleteReply, Exception>;

    /// Handles a request to return info on open comms.
    ///
    /// Docs: https://jupyter-client.readthedocs.io/en/stable/messaging.html#comm-info
    fn handle_comm_info_request(&self, req: CommInfoRequest) -> Result<CommInfoReply, Exception>;
}
