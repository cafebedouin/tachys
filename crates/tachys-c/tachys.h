/* tachys: the wallet's one call, from C (crates/tachys-c). Dedicated to the public domain (CC0-1.0).
 *
 * One function runs one command, exactly as the `tachys` program would, in the caller's process:
 *
 *     const char *args[] = { "tachys", "balance", "--state", dir, "--json" };
 *     tachys_answer *a = tachys_call(args, 5, NULL, 0);
 *     if (a && a->code == 0) use(a->out);        // the one JSON object (docs/json.md)
 *     tachys_free(a);
 *
 * It blocks until the command has ended. It may be called from several threads at once; each call has its own
 * network and its own answer. Nothing is written to the process's stdout, and the process is never ended: a panic
 * inside the wallet (a fault in it) comes back as code 70, after Rust has written its one line about it to the
 * process's stderr. A stack overflow or an abort inside it would still end the
 * process: nothing in a library can stop that.
 */
#ifndef TACHYS_H
#define TACHYS_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct tachys_answer {
    /* 0: done; 1: refused, failed or not yet so; 2: misuse; 70: the command panicked (a fault in tachys) */
    int32_t code;
    /* what the program would have written to stdout: with --json among the arguments, the one JSON object.
     * UTF-8, ends with a 0 byte, never NULL. */
    char *out;
    /* what the program would have written to stderr: sentences for a person. Never decide anything on them. */
    char *err;
} tachys_answer;

/* Runs one command.
 * args:  n_args strings, as on the command line, the program's name ("tachys") first. UTF-8, each ending with a 0 byte.
 * input: n_input strings, the lines the command reads where the program would read stdin: the password for
 *        --pass-stdin, then a seed phrase for --mnemonic-file -. May be NULL when n_input is 0. The library keeps no
 *        copy after the call; the caller's own copies are the caller's to wipe.
 * Returns an answer to be given back to tachys_free, or NULL if there is no memory for one. */
tachys_answer *tachys_call(const char *const *args, size_t n_args, const char *const *input, size_t n_input);

/* Frees an answer and its strings. NULL is allowed. An answer is freed once. */
void tachys_free(tachys_answer *answer);

#ifdef __cplusplus
}
#endif

#endif
