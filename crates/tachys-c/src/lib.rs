//! The wallet's one call (`tachys::call`) for a program that is not written in Rust: a C interface (`tachys.h`), and
//! the same call for a JVM through the JNI (`jni.rs`), which is how the phone app runs the core in its own process.
//!
//! Both run the command on a thread of its own with the stack the program's main thread has. A thread a JVM or an
//! Apple app calls from may have an eighth of that or less, and the wallet has only ever been run with the whole.
#![deny(unsafe_op_in_unsafe_fn)]

use std::ffi::{c_char, CStr, CString};
use zeroize::Zeroize;

#[cfg(any(target_os = "android", feature = "jni"))]
mod jni;

/// What a Linux or Android process gives its main thread, which is where the `tachys` program runs a command.
const STACK: usize = 8 << 20;

/// One call on a thread of its own. The lines of `input` (a password, a seed phrase) are wiped when it has ended.
pub fn call(args: Vec<String>, mut input: Vec<String>) -> tachys::Answer {
    let ran = std::thread::Builder::new().name("tachys".into()).stack_size(STACK).spawn(move || {
        let answer = tachys::call(&args, &input);
        input.iter_mut().for_each(Zeroize::zeroize);
        answer
    });
    match ran.map(|thread| thread.join()) {
        Ok(Ok(answer)) => answer,
        // `tachys::call` catches a command's panic itself and answers 70; this is one outside the command
        Ok(Err(_)) => failed(70, "the call panicked: a fault in tachys, not in what was asked"),
        Err(e) => failed(1, &format!("no thread to run the command on: {e}")),
    }
}

/// An answer for a call that never reached a command, in the shape a command's own failure has (`docs/json.md`).
fn failed(code: i32, why: &str) -> tachys::Answer {
    tachys::Answer { code, out: format!("{{\"ok\":false,\"v\":1,\"exit\":{code},\"error\":{}}}\n", quoted(why)), err: format!("{why}\n") }
}
/// `s` as a JSON string. What is said here is this file's own words and an operating system's error: no serializer is needed for it.
fn quoted(s: &str) -> String {
    let mut q = String::from("\"");
    for c in s.chars() { match c { '"' => q.push_str("\\\""), '\\' => q.push_str("\\\\"), c if (c as u32) < 0x20 => q.push_str(&format!("\\u{:04x}", c as u32)), c => q.push(c) } }
    q + "\""
}

/// See `tachys.h`.
#[repr(C)]
pub struct tachys_answer { pub code: i32, pub out: *mut c_char, pub err: *mut c_char }

/// `n` C strings as Rust's, or which of them is not one.
///
/// # Safety
/// `list` is null or points to `n` pointers, each null or to bytes ending with a 0.
unsafe fn strings(list: *const *const c_char, n: usize, what: &str) -> Result<Vec<String>, String> {
    if n == 0 { return Ok(vec![]) }
    if list.is_null() { return Err(format!("{what}: no list, though it is said to hold {n}")) }
    let mut all = Vec::with_capacity(n);
    for i in 0..n {
        // SAFETY: the caller's word that `list` holds `n` pointers
        let one = unsafe { *list.add(i) };
        if one.is_null() { all.iter_mut().for_each(Zeroize::zeroize); return Err(format!("{what} {i} is NULL")) }
        // SAFETY: the caller's word that each ends with a 0
        match unsafe { CStr::from_ptr(one) }.to_str() {
            Ok(s) => all.push(s.to_string()),
            // which one, never what it held: it may be a password
            Err(_) => { all.iter_mut().for_each(Zeroize::zeroize); return Err(format!("{what} {i} is not UTF-8")) }
        }
    }
    Ok(all)
}
/// A string for C. One holding a 0 byte (a node's words, quoted in a sentence) loses those bytes rather than its end.
fn for_c(s: String) -> *mut c_char { CString::new(s.into_bytes().into_iter().filter(|&b| b != 0).collect::<Vec<u8>>()).unwrap_or_default().into_raw() }

/// Runs one command. See `tachys.h`.
///
/// # Safety
/// `args` points to `n_args` C strings and `input` to `n_input` (either may be null if its count is 0).
#[no_mangle]
pub unsafe extern "C" fn tachys_call(args: *const *const c_char, n_args: usize, input: *const *const c_char, n_input: usize) -> *mut tachys_answer {
    // nothing may unwind into C: a panic here, outside the command, is answered like one inside it
    let answer = std::panic::catch_unwind(|| {
        // SAFETY: the caller's word, as this function's own contract says
        match (unsafe { strings(args, n_args, "argument") }, unsafe { strings(input, n_input, "input line") }) {
            (Ok(args), Ok(input)) => call(args, input),
            (Err(why), Ok(mut read)) | (Ok(mut read), Err(why)) => { read.iter_mut().for_each(Zeroize::zeroize); failed(2, &why) }
            (Err(why), Err(_)) => failed(2, &why),
        }
    }).unwrap_or_else(|_| failed(70, "the call panicked: a fault in tachys, not in what was asked"));
    Box::into_raw(Box::new(tachys_answer { code: answer.code, out: for_c(answer.out), err: for_c(answer.err) }))
}

/// Frees an answer. See `tachys.h`.
///
/// # Safety
/// `answer` is null or came from `tachys_call` and has not been freed.
#[no_mangle]
pub unsafe extern "C" fn tachys_free(answer: *mut tachys_answer) {
    if answer.is_null() { return }
    // SAFETY: made by `Box::into_raw` in `tachys_call`, and its strings by `CString::into_raw`
    let a = unsafe { Box::from_raw(answer) };
    for s in [a.out, a.err] {
        if s.is_null() { continue }
        // an answer may be a seed phrase (`wallet mnemonic`): wiped before the memory goes back
        let mut bytes = unsafe { CString::from_raw(s) }.into_bytes();
        bytes.zeroize();
    }
}
