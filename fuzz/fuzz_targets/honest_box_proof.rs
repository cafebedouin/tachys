// The honest proof in fuzz/seeds/box_proof/honest is a lookup of these three keys (the second is not in the tree)
// in a tree of three boxes with this root: made as the wallet's tests make one (crates/tachys-cli/src/tests.rs, proven_lookup).
pub const ROOT: [u8; 33] = [
    2, 23, 141, 35, 224, 91, 23, 38, 31, 138, 143, 35, 29, 98, 216, 184, 25, 24, 79, 130, 31, 233,
    97, 234, 111, 89, 194, 123, 1, 203, 55, 204, 2,
];
pub const KEYS: [[u8; 32]; 3] = [
    [
        96, 127, 224, 50, 0, 194, 25, 195, 90, 56, 251, 123, 28, 111, 255, 234, 165, 54, 139, 243,
        207, 166, 55, 82, 1, 13, 79, 125, 68, 237, 168, 35,
    ],
    [
        9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9,
        9, 9,
    ],
    [
        167, 68, 141, 218, 87, 135, 135, 195, 208, 88, 248, 244, 253, 74, 65, 26, 72, 12, 41, 21,
        197, 249, 111, 104, 100, 90, 181, 143, 89, 213, 235, 64,
    ],
];
