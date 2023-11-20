//
// util.rs
//
// Copyright (C) 2022 Posit Software, PBC. All rights reserved.
//
//

use std::os::raw::c_char;

use harp::object::RObject;
use libR_sys::*;

/// Shows a message in the Positron frontend
#[harp::register]
pub unsafe extern "C" fn ps_log_error(message: SEXP) -> SEXP {
    let message = RObject::view(message).to::<String>();
    if let Ok(message) = message {
        log::error!("{}", message);
    }

    Ok(R_NilValue)
}

#[harp::register]
pub unsafe extern "C" fn ps_object_id(object: SEXP) -> SEXP {
    let value = format!("{:p}", object);
    return Ok(Rf_mkString(value.as_ptr() as *const c_char));
}
