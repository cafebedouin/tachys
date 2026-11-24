//! The same call for a JVM: `org.tachys.wallet.Native.call(args, input)` (`android/app/src/main/kotlin/…/Native.kt`).
//!
//! Only arrays of bytes cross. Strings are UTF-8 and each ends with a 0 byte, so a list is its strings one after
//! another and an empty list is no bytes. (The JNI's own strings are a modified UTF-8 that is not UTF-8 for every
//! character; arrays of bytes are what they are, and an array can be wiped where a `String` cannot.) The answer is
//! the code and the length of `out` as two big-endian 32-bit numbers, then `out`, then `err`.

use std::ffi::c_void;
use zeroize::Zeroize;

// The JNI as `jni.h` has it, for the four functions used here and no more. A native method is handed a pointer to a
// pointer to a table of functions, and a function's place in that table is fixed by the JNI's specification: these
// four are where both the JDK's `jni.h` and the Android NDK's put them (four reserved places, then `GetVersion`, …).
// The app's tests call through them in a JVM on the build machine (`NativeTest.kt`).
type Env = *mut *const *const c_void;
type Array = *mut c_void;
const GET_ARRAY_LENGTH: usize = 171;
const NEW_BYTE_ARRAY: usize = 176;
const GET_BYTE_ARRAY_REGION: usize = 200;
const SET_BYTE_ARRAY_REGION: usize = 208;
type GetArrayLength = unsafe extern "system" fn(Env, Array) -> i32;
type NewByteArray = unsafe extern "system" fn(Env, i32) -> Array;
type GetByteArrayRegion = unsafe extern "system" fn(Env, Array, i32, i32, *mut i8);
type SetByteArrayRegion = unsafe extern "system" fn(Env, Array, i32, i32, *const i8);

/// The function at one place of the JNI's table, or `None` if the JVM left that place empty.
///
/// # Safety
/// `env` is the JNI environment a JVM handed a native method, and `F` the type of the function at `place`.
unsafe fn function<F: Copy>(env: Env, place: usize) -> Option<F> {
    const { assert!(std::mem::size_of::<F>() == std::mem::size_of::<*const c_void>()) };
    // SAFETY: the table a JVM hands over holds every place the specification names
    let entry: *const c_void = unsafe { *(*env).add(place) };
    // SAFETY: not null, and the caller's word for its type
    if entry.is_null() { None } else { Some(unsafe { std::mem::transmute_copy::<*const c_void, F>(&entry) }) }
}

/// The bytes of a Java array, or `None` if the JVM would not hand them over.
///
/// # Safety
/// `env` is the JNI environment of the calling thread and `array` a live `byte[]` (or null).
unsafe fn bytes(env: Env, array: Array) -> Option<Vec<u8>> {
    if array.is_null() { return Some(vec![]) }
    // SAFETY: the places and the types are the specification's
    let (length, region) = unsafe { (function::<GetArrayLength>(env, GET_ARRAY_LENGTH)?, function::<GetByteArrayRegion>(env, GET_BYTE_ARRAY_REGION)?) };
    // SAFETY: `array` is live for this call
    let n = unsafe { length(env, array) };
    let mut b = vec![0u8; usize::try_from(n).ok()?];
    // SAFETY: `b` holds exactly the `n` bytes asked for
    unsafe { region(env, array, 0, n, b.as_mut_ptr() as *mut i8) };
    Some(b)
}
/// The strings of a list as it crosses: each ends with a 0 byte. `None` if one is not UTF-8.
fn list(mut b: Vec<u8>) -> Option<Vec<String>> {
    let all: Option<Vec<String>> = b.split_inclusive(|&c| c == 0).map(|s| std::str::from_utf8(s.strip_suffix(&[0]).unwrap_or(s)).ok().map(str::to_string)).collect();
    b.zeroize();
    all
}
/// An answer as it crosses back.
fn packed(a: tachys::Answer) -> Vec<u8> {
    let mut b = Vec::with_capacity(8 + a.out.len() + a.err.len());
    b.extend_from_slice(&a.code.to_be_bytes());
    b.extend_from_slice(&(a.out.len() as u32).to_be_bytes());
    b.extend_from_slice(a.out.as_bytes());
    b.extend_from_slice(a.err.as_bytes());
    let (mut out, mut err) = (a.out, a.err);
    out.zeroize(); err.zeroize();
    b
}

/// `Native.call(args: ByteArray, input: ByteArray): ByteArray?`. Null only if the JVM has no memory for the answer.
///
/// # Safety
/// Called by a JVM as the native method it is declared as.
#[no_mangle]
pub unsafe extern "system" fn Java_org_tachys_wallet_Native_call(env: Env, _class: *mut c_void, args: Array, input: Array) -> Array {
    // nothing may unwind into the JVM
    let answer = std::panic::catch_unwind(|| {
        // SAFETY: `env` and the arrays are the JVM's own, live for this call
        let (args, input) = unsafe { (bytes(env, args), bytes(env, input)) };
        match (args.and_then(list), input.and_then(list)) {
            (Some(args), Some(input)) => crate::call(args, input),
            (_, read) => { if let Some(mut lines) = read { lines.iter_mut().for_each(Zeroize::zeroize); } crate::failed(2, "the arguments or the input lines are not UTF-8") }
        }
    }).unwrap_or_else(|_| crate::failed(70, "the call panicked: a fault in tachys, not in what was asked"));
    let mut b = packed(answer);
    // SAFETY: as above; a null from `NewByteArray` is an OutOfMemoryError already raised in the JVM
    let made = unsafe {
        match (function::<NewByteArray>(env, NEW_BYTE_ARRAY), function::<SetByteArrayRegion>(env, SET_BYTE_ARRAY_REGION), i32::try_from(b.len())) {
            (Some(new), Some(set), Ok(n)) => { let array = new(env, n); if !array.is_null() { set(env, array, 0, n, b.as_ptr() as *const i8); } array }
            _ => std::ptr::null_mut(),
        }
    };
    // the answer may be a seed phrase (`wallet mnemonic`)
    b.zeroize();
    made
}
