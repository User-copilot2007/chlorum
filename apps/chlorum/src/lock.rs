//! Checking a password for the lock screen through PAM (service
//! "chlorum-lock"), which on Chlorum asks systemd-homed. A few lines of
//! direct calls instead of a library, so nothing extra is needed to build.

use std::ffi::{CStr, CString, c_char, c_int, c_void};

#[repr(C)]
struct PamMessage {
    msg_style: c_int,
    msg: *const c_char,
}

#[repr(C)]
struct PamResponse {
    resp: *mut c_char,
    resp_retcode: c_int,
}

#[repr(C)]
struct PamConv {
    conv: extern "C" fn(c_int, *mut *const PamMessage, *mut *mut PamResponse, *mut c_void) -> c_int,
    appdata_ptr: *mut c_void,
}

const PAM_SUCCESS: c_int = 0;
const PAM_BUF_ERR: c_int = 5;
const PAM_CONV_ERR: c_int = 19;
const PAM_PROMPT_ECHO_OFF: c_int = 1;
const PAM_PROMPT_ECHO_ON: c_int = 2;

#[link(name = "pam")]
unsafe extern "C" {
    fn pam_start(service: *const c_char, user: *const c_char, conv: *const PamConv, pamh: *mut *mut c_void) -> c_int;
    fn pam_authenticate(pamh: *mut c_void, flags: c_int) -> c_int;
    fn pam_acct_mgmt(pamh: *mut c_void, flags: c_int) -> c_int;
    fn pam_end(pamh: *mut c_void, status: c_int) -> c_int;
    fn pam_strerror(pamh: *mut c_void, errnum: c_int) -> *const c_char;
}

unsafe extern "C" {
    fn calloc(n: usize, size: usize) -> *mut c_void;
    fn strdup(s: *const c_char) -> *mut c_char;
}

/// Answers every password question with the typed password, once.
extern "C" fn conversation(
    n: c_int,
    msgs: *mut *const PamMessage,
    resp: *mut *mut PamResponse,
    data: *mut c_void,
) -> c_int {
    if n <= 0 || msgs.is_null() || resp.is_null() {
        return PAM_CONV_ERR;
    }
    unsafe {
        let out = calloc(n as usize, std::mem::size_of::<PamResponse>()) as *mut PamResponse;
        if out.is_null() {
            return PAM_BUF_ERR;
        }
        let password = data as *const c_char;
        for i in 0..n as usize {
            let m = *msgs.add(i);
            let style = if m.is_null() { 0 } else { (*m).msg_style };
            if style == PAM_PROMPT_ECHO_OFF || style == PAM_PROMPT_ECHO_ON {
                (*out.add(i)).resp = strdup(password);
            }
        }
        *resp = out;
    }
    PAM_SUCCESS
}

pub fn authenticate(user: &str, password: &str) -> Result<(), String> {
    let service = CString::new("chlorum-lock").unwrap();
    let user = CString::new(user).map_err(|_| "Wrong password.")?;
    let password = CString::new(password).map_err(|_| "Wrong password.")?;
    let conv = PamConv { conv: conversation, appdata_ptr: password.as_ptr() as *mut c_void };
    let mut h: *mut c_void = std::ptr::null_mut();
    unsafe {
        let r = pam_start(service.as_ptr(), user.as_ptr(), &conv, &mut h);
        if r != PAM_SUCCESS {
            return Err("The password check is not available.".into());
        }
        let mut r = pam_authenticate(h, 0);
        if r == PAM_SUCCESS {
            r = pam_acct_mgmt(h, 0);
        }
        if r != PAM_SUCCESS {
            eprintln!("chlorum lock: PAM answered {r} for the password");
        }
        let result = if r == PAM_SUCCESS {
            Ok(())
        } else if r == 7 || r == 10 {
            // PAM_AUTH_ERR, PAM_USER_UNKNOWN
            Err("Wrong password. Try again.".into())
        } else {
            let e = pam_strerror(h, r);
            Err(if e.is_null() { "Could not check the password.".into() } else { CStr::from_ptr(e).to_string_lossy().to_string() })
        };
        pam_end(h, r);
        result
    }
}
