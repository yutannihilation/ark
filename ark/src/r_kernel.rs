/*
 * r_kernel.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use crate::r_request::RRequest;
use amalthea::socket::iopub::IOPubMessage;
use amalthea::wire::exception::Exception;
use amalthea::wire::execute_input::ExecuteInput;
use amalthea::wire::execute_reply::ExecuteReply;
use amalthea::wire::execute_reply_exception::ExecuteReplyException;
use amalthea::wire::execute_request::ExecuteRequest;
use amalthea::wire::execute_response::ExecuteResponse;
use amalthea::wire::execute_result::ExecuteResult;
use amalthea::wire::jupyter_message::Status;
use extendr_api::prelude::*;
use log::{debug, trace, warn};
use serde_json::json;
use std::sync::mpsc::{Sender, SyncSender};

/// Represents the Rust state of the R kernel
pub struct RKernel {
    pub execution_count: u32,
    iopub: SyncSender<IOPubMessage>,
    console: Sender<Option<String>>,
    initializer: Sender<RKernelInfo>,
    output: String,
    response_sender: Option<Sender<ExecuteResponse>>,
    banner: String,
    initializing: bool,
}

/// Represents kernel metadata (available after the kernel has fully started)
pub struct RKernelInfo {
    pub version: String,
    pub banner: String,
}

impl RKernel {
    /// Create a new R kernel instance
    pub fn new(
        iopub: SyncSender<IOPubMessage>,
        console: Sender<Option<String>>,
        initializer: Sender<RKernelInfo>,
    ) -> Self {
        Self {
            iopub: iopub,
            execution_count: 0,
            console: console,
            output: String::new(),
            banner: String::new(),
            initializing: true,
            initializer: initializer,
            response_sender: None,
        }
    }

    /// Completes the kernel's initialization
    pub fn complete_intialization(&mut self) {
        if self.initializing {
            let ver = R!(R.version.string).unwrap();
            let ver_str = ver.as_str().unwrap().to_string();
            let kernel_info = RKernelInfo {
                version: ver_str.clone(),
                banner: self.banner.clone(),
            };
            debug!("Sending kernel info: {}", ver_str);
            self.initializer.send(kernel_info).unwrap();
            self.initializing = false;
        } else {
            warn!("Initialization already complete!");
        }
    }

    /// Service an execution request from the front end
    pub fn fulfill_request(&mut self, req: &RRequest) {
        match req {
            RRequest::ExecuteCode(req, sender) => {
                let sender = sender.clone();
                self.handle_execute_request(req, sender);
            }
            RRequest::Shutdown(_) => {
                if let Err(err) = self.console.send(None) {
                    warn!("Error sending shutdown message to console: {}", err);
                }
            }
        }
    }

    pub fn handle_execute_request(
        &mut self,
        req: &ExecuteRequest,
        sender: Sender<ExecuteResponse>,
    ) {
        self.output = String::new();
        self.response_sender = Some(sender);

        // Increment counter if we are storing this execution in history
        if req.store_history {
            self.execution_count = self.execution_count + 1;
        }

        // If the code is not to be executed silently, re-broadcast the
        // execution to all frontends
        if !req.silent {
            if let Err(err) = self.iopub.send(IOPubMessage::ExecuteInput(ExecuteInput {
                code: req.code.clone(),
                execution_count: self.execution_count,
            })) {
                warn!(
                    "Could not broadcast execution input {} to all front ends: {}",
                    self.execution_count, err
                );
            }
        }

        // Send the code to the R console to be evaluated
        self.console.send(Some(req.code.clone())).unwrap();
    }

    /// Converts a data frame to HTML
    pub fn to_html(frame: &Robj) -> String {
        let names = frame.names().unwrap();
        let mut th = String::from("<tr>");
        for i in names {
            let h = format!("<th>{}</th>", i);
            th.push_str(h.as_str());
        }
        th.push_str("</tr>");
        let mut body = String::new();
        for i in 1..5 {
            body.push_str("<tr>");
            for j in 1..(frame.len() + 1) {
                trace!("formatting value at {}, {}", i, j);
                if let Ok(col) = frame.index(i) {
                    if let Ok(val) = col.index(j) {
                        if let Ok(s) = call!("toString", val) {
                            body.push_str(
                                format!("<td>{}</td>", String::from_robj(&s).unwrap()).as_str(),
                            )
                        }
                    }
                }
            }
            body.push_str("</tr>");
        }
        format!(
            "<table><thead>{}</thead><tbody>{}</tbody></table>",
            th, body
        )
    }

    /// Report an incomplete request to the front end
    pub fn report_incomplete_request(&self, req: &RRequest) {
        let code = match req {
            RRequest::ExecuteCode(req, _) => req.code.clone(),
            _ => String::new(),
        };
        if let Some(sender) = self.response_sender.as_ref() {
            let reply = ExecuteReplyException {
                status: Status::Error,
                execution_count: self.execution_count,
                exception: Exception {
                    ename: "IncompleteInput".to_string(),
                    evalue: format!("Code fragment is not complete: {}", code),
                    traceback: vec![],
                },
            };
            if let Err(err) = sender.send(ExecuteResponse::ReplyException(reply)) {
                warn!("Error sending incomplete reply: {}", err);
            }
        }
    }

    /// Finishes the active execution request
    pub fn finish_request(&self) {
        let output = self.output.clone();

        // Look up computation result
        let mut data = serde_json::Map::new();
        data.insert("text/plain".to_string(), json!(output));
        trace!("Formatting value");
        let last = R!(.Last.value).unwrap();
        if last.is_frame() {
            data.insert("text/html".to_string(), json!(RKernel::to_html(&last)));
        }

        trace!("Sending kernel output: {}", self.output);
        if let Err(err) = self.iopub.send(IOPubMessage::ExecuteResult(ExecuteResult {
            execution_count: self.execution_count,
            data: serde_json::Value::Object(data),
            metadata: json!({}),
        })) {
            warn!(
                "Could not publish result of statement {} on iopub: {}",
                self.execution_count, err
            );
        }

        // Send the reply to the front end
        if let Some(sender) = &self.response_sender {
            sender
                .send(ExecuteResponse::Reply(ExecuteReply {
                    status: Status::Ok,
                    execution_count: self.execution_count,
                    user_expressions: json!({}),
                }))
                .unwrap();
        }
    }

    /// Called from R when console data is written
    pub fn write_console(&mut self, content: &str, otype: i32) {
        debug!("Write console {} from R: {}", otype, content);
        if self.initializing {
            // During init, consider all output to be part of the startup banner
            self.banner.push_str(content);
        } else {
            // Afterwards (during normal REPL), accumulate output internally
            // until R is finished executing
            self.output.push_str(content);
        }
    }
}
