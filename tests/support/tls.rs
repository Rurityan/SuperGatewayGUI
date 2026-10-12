//! UI automation only for the unique CA generated in an isolated test directory.
//! Production trust actions retain the Windows certificate warning.
#![allow(unsafe_op_in_unsafe_fn)]
use gateway::{native::wide, tls};
use std::{
    path::Path,
    ptr::{null, null_mut},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::{HWND, LPARAM},
    Security::Cryptography::*,
    UI::WindowsAndMessaging::*,
};
pub fn ca_der(root: &Path) -> Vec<u8> {
    let text = wide(std::fs::read_to_string(tls::ca_path(root)).unwrap());
    unsafe {
        let mut length = 0;
        assert_ne!(
            CryptStringToBinaryW(
                text.as_ptr(),
                0,
                CRYPT_STRING_BASE64HEADER,
                null_mut(),
                &mut length,
                null_mut(),
                null_mut()
            ),
            0
        );
        let mut bytes = vec![0; length as usize];
        assert_ne!(
            CryptStringToBinaryW(
                text.as_ptr(),
                0,
                CRYPT_STRING_BASE64HEADER,
                bytes.as_mut_ptr(),
                &mut length,
                null_mut(),
                null_mut()
            ),
            0
        );
        bytes
    }
}
pub fn trust(root: &Path) -> gateway::Result<()> {
    set_trust(root, true)
}
pub fn untrust(root: &Path) -> gateway::Result<()> {
    set_trust(root, false)
}
fn set_trust(root: &Path, trusted: bool) -> gateway::Result<()> {
    let der = ca_der(root);
    let expected = unsafe {
        let cert = CertCreateCertificateContext(X509_ASN_ENCODING, der.as_ptr(), der.len() as u32);
        assert!(!cert.is_null());
        let length = CertGetNameStringW(
            cert,
            CERT_NAME_SIMPLE_DISPLAY_TYPE,
            0,
            null(),
            null_mut(),
            0,
        );
        let mut name = vec![0u16; length as usize];
        CertGetNameStringW(
            cert,
            CERT_NAME_SIMPLE_DISPLAY_TYPE,
            0,
            null(),
            name.as_mut_ptr(),
            length,
        );
        CertFreeCertificateContext(cert);
        String::from_utf16_lossy(&name[..name.len() - 1])
    };
    assert!(expected.starts_with("SuperGateway GUI Local CA "));
    struct Dialog {
        expected: String,
        confirmed: bool,
        cancel: bool,
    }
    unsafe extern "system" fn children(hwnd: HWND, l: LPARAM) -> i32 {
        let text = &mut *(l as *mut String);
        let mut buf = vec![0u16; GetWindowTextLengthW(hwnd) as usize + 1];
        GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
        text.push_str(&String::from_utf16_lossy(&buf));
        1
    }
    unsafe extern "system" fn windows(hwnd: HWND, l: LPARAM) -> i32 {
        let state = &mut *(l as *mut Dialog);
        let mut pid = 0;
        GetWindowThreadProcessId(hwnd, &mut pid);
        if pid != std::process::id() {
            return 1;
        }
        let mut text = String::new();
        EnumChildWindows(hwnd, Some(children), &mut text as *mut String as isize);
        if text.contains(&state.expected) && !GetDlgItem(hwnd, 6).is_null() {
            PostMessageW(hwnd, WM_COMMAND, if state.cancel { 7 } else { 6 }, 0);
            state.confirmed = true;
            return 0;
        }
        1
    }
    let stop = Arc::new(AtomicBool::new(false));
    let done = stop.clone();
    let automation = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(15);
        let mut dialog = Dialog {
            expected,
            confirmed: false,
            cancel: false,
        };
        while !done.load(Ordering::Acquire) && !dialog.confirmed {
            dialog.cancel = Instant::now() > deadline;
            unsafe {
                EnumWindows(Some(windows), &mut dialog as *mut Dialog as isize);
            }
            thread::sleep(Duration::from_millis(30));
        }
    });
    let result = tls::set_trust(root, trusted);
    stop.store(true, Ordering::Release);
    automation.join().unwrap();
    result
}
